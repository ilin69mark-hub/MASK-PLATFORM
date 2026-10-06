//! DXF writer. AutoCAD R12 target.
//!
//! R12 rather than a modern release on purpose: it is the most widely readable
//! DXF there is. ArtCAM, laser software, older plotters and a plain text editor
//! all open it, whereas R2010+ drops support from half of them.
//!
//! Consequence to live with: R12 has no LWPOLYLINE, so every curve is written as
//! a classical POLYLINE with explicit VERTEX entities, and curves are flattened
//! at the plotter's `curve_tolerance_mm`.

use mask_kernel::{Contour, Shape, Shapes, CANON_COORD_DP};

/// One named layer inside the DXF, matching the entity types we emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxfLayer {
    /// Cut geometry.
    Cut,
    /// Registration marks and printable-area outlines.
    Registration,
    /// Text and metadata.
    Info,
}

impl DxfLayer {
    pub const CUT_NAME: &'static str = "CUT";
    pub const REGISTRATION_NAME: &'static str = "REGISTRATION";
    pub const INFO_NAME: &'static str = "INFO";

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Cut => Self::CUT_NAME,
            Self::Registration => Self::REGISTRATION_NAME,
            Self::Info => Self::INFO_NAME,
        }
    }
}

/// Geometry to write, grouped by DXF layer.
#[derive(Debug, Clone, Default)]
pub struct DxfContent {
    pub cut: Shapes,
    pub registration: Shapes,
}

/// Curve flattening tolerance, in millimetres.
///
/// Taken from the plotter profile: a curve the knife cannot hold precisely must
/// not be advertised in a CAD file as if it were exact.
#[must_use]
pub fn curve_tolerance_mm(plotter_curve_tolerance_mm: f64) -> f64 {
    if plotter_curve_tolerance_mm > 0.0 {
        plotter_curve_tolerance_mm
    } else {
        mask_kernel::COLLINEAR_MM
    }
}

/// Write an R12 DXF document.
///
/// Line endings are CRLF because that is what the R12 specification requires;
/// every other artifact in the system uses `\n` (ADR-0004).
#[must_use]
pub fn to_dxf(content: &DxfContent, tolerance_mm: f64) -> String {
    let mut out = String::with_capacity(8192);

    // ── HEADER ────────────────────────────────────────────────────────
    pair(&mut out, 999, "MASK PLATFORM production file");
    // The R12 target has no arc entity for a knife to follow, so curves arrive as
    // POLYLINE vertices. Without this line a reader cannot tell a tessellated
    // circle from a circle, and every point in the file looks like original
    // geometry. The tolerance is the blade's, so the approximation belongs to the
    // machine that has to cut it.
    pair(
        &mut out,
        999,
        &format!(
            "curves flattened at {tolerance_mm:.4}mm chord tolerance (DXF R12 carries no arc entity)"
        ),
    );
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "HEADER");
    // $ACADVER = AC1009 (R12)
    pair(&mut out, 9, "$ACADVER");
    pair(&mut out, 1, "AC1009");
    // $INSUNITS = 4 (millimetres). Without this a CAD app assumes its own unit
    // and the mask arrives scaled by an arbitrary factor.
    pair(&mut out, 9, "$INSUNITS");
    pair(&mut out, 70, "4");
    pair(&mut out, 9, "$EXTMIN");
    pair(&mut out, 10, &num(min_of(&content.cut).0));
    pair(&mut out, 20, &num(min_of(&content.cut).1));
    pair(&mut out, 9, "$EXTMAX");
    pair(&mut out, 10, &num(max_of(&content.cut).0));
    pair(&mut out, 20, &num(max_of(&content.cut).1));
    pair(&mut out, 0, "ENDSEC");

    // ── TABLES: layer table ───────────────────────────────────────────
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "TABLES");
    pair(&mut out, 0, "TABLE");
    pair(&mut out, 2, "LAYER");
    pair(&mut out, 70, "3");
    for layer in [DxfLayer::Cut, DxfLayer::Registration, DxfLayer::Info] {
        pair(&mut out, 0, "LAYER");
        pair(&mut out, 2, layer.name());
        pair(&mut out, 70, "0");
        pair(&mut out, 62, "7"); // colour 7 = white/black depending on background
        pair(&mut out, 6, "CONTINUOUS");
    }
    pair(&mut out, 0, "ENDTAB");
    pair(&mut out, 0, "ENDSEC");

    // ── ENTITIES ──────────────────────────────────────────────────────
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "ENTITIES");
    emit_polylines(&mut out, &content.cut, DxfLayer::Cut, tolerance_mm);
    emit_polylines(
        &mut out,
        &content.registration,
        DxfLayer::Registration,
        tolerance_mm,
    );
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "EOF");

    out
}

fn emit_polylines(out: &mut String, shapes: &Shapes, layer: DxfLayer, tolerance_mm: f64) {
    for shape in &shapes.shapes {
        emit_polyline(out, &shape.outer, layer, tolerance_mm);
        for hole in &shape.holes {
            emit_polyline(out, hole, layer, tolerance_mm);
        }
    }
}

fn emit_polyline(out: &mut String, contour: &Contour, layer: DxfLayer, tolerance_mm: f64) {
    let points = flatten(contour, tolerance_mm);
    if points.len() < 2 {
        return;
    }

    pair(out, 0, "POLYLINE");
    pair(out, 8, layer.name());
    pair(out, 66, "1"); // vertices follow
    pair(out, 70, if contour.closed { "1" } else { "0" }); // 1 = closed
                                                           // Elevation and extrusion omitted: a flat mask does not need them, and
                                                           // omitted fields are safer than guessed ones.
    for (i, p) in points.iter().enumerate() {
        pair(out, 0, "VERTEX");
        pair(out, 8, layer.name());
        pair(out, 10, &num(p.x));
        pair(out, 20, &num(p.y));
        pair(out, 30, "0.0");
        if i + 1 == points.len() {
            // A 70 flag of 1 on the last vertex repeats the first point.
            pair(out, 70, if contour.closed { "1" } else { "0" });
        }
    }
    pair(out, 0, "SEQEND");
}

/// Drop points that add nothing at the given tolerance.
///
/// This is the flattening step: DXF R12 has no curves, so a bezier must arrive
/// already tessellated. Collapsing duplicates here rather than trusting the
/// writer keeps the vertex count honest.
fn flatten(contour: &Contour, tolerance_mm: f64) -> Vec<mask_kernel::Point> {
    // Tessellate first. This function used to walk `contour.points` and call
    // itself flatten, and this module's header claimed curves were flattened at the
    // plotter tolerance. They were not: a circle of four quarter arcs was written
    // as four vertices, the inscribed square, in a file that said it had been
    // flattened. The claim in the documentation was false and so was the file.
    let tessellated = contour.flattened(tolerance_mm);
    let mut out: Vec<mask_kernel::Point> = Vec::with_capacity(tessellated.points.len());
    for p in &tessellated.points {
        if let Some(last) = out.last() {
            if last.distance_to(*p) <= tolerance_mm.max(mask_kernel::COLLINEAR_MM) {
                continue;
            }
        }
        out.push(*p);
    }

    if contour.closed && out.len() > 1 {
        while out.len() > 1 {
            let first = out[0];
            let last = *out.last().expect("non-empty");
            if last.distance_to(first) <= tolerance_mm.max(mask_kernel::COLLINEAR_MM) {
                out.pop();
            } else {
                break;
            }
        }
    }
    out
}

/// One group-code / value pair.
///
/// Every newline is CRLF, including the one separating the code from the value:
/// a DXF reader that sees a bare LF between them parses the next group as
/// garbage. Every other artifact in this system uses LF (ADR-0004), which makes
/// this the one place the difference matters.
fn pair(out: &mut String, code: u16, value: &str) {
    out.push_str(&code.to_string());
    out.push_str("\r\n");
    out.push_str(value);
    out.push_str("\r\n");
}

fn num(value: f64) -> String {
    format!("{value:.prec$}", prec = CANON_COORD_DP)
}

fn min_of(shapes: &Shapes) -> (f64, f64) {
    let (x, y, _, _) = mask_kernel::bounding_box(shapes);
    (x, y)
}

fn max_of(shapes: &Shapes) -> (f64, f64) {
    let (x, y, w, h) = mask_kernel::bounding_box(shapes);
    (x + w, y + h)
}

/// Every numeric literal in a document, for precision assertions.
#[cfg(test)]
fn numeric_literals(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_digit()
            || (c == '.' && i + 1 < bytes.len() && (bytes[i + 1] as char).is_ascii_digit())
        {
            let start = i;
            while i < bytes.len()
                && ((bytes[i] as char).is_ascii_digit() || bytes[i] as char == '.')
            {
                i += 1;
            }
            let token = &text[start..i];
            if token.contains('.') {
                out.push(token);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Read a DXF back into geometry.
///
/// Only the subset this writer emits is supported: POLYLINE with VERTEX on the
/// CUT and REGISTRATION layers. Anything else is an error rather than a silent
/// skip, because a partial read would make a round-trip check meaningless.
pub fn from_dxf(text: &str) -> Result<DxfContent, String> {
    let mut lines: Vec<&str> = text.lines().collect();
    // Tolerate either line ending.
    lines = lines.iter().map(|l| l.trim_end_matches('\r')).collect();

    let mut content = DxfContent::default();
    let mut i = 0usize;

    #[derive(PartialEq)]
    enum Mode {
        Idle,
        InCut,
        InRegistration,
    }
    let mut mode = Mode::Idle;

    while i + 1 < lines.len() {
        let code = lines[i].trim();
        let value = lines[i + 1];
        i += 2;

        match code {
            "0" => match value {
                "SECTION" => {}
                "LAYER" => {}
                "POLYLINE" => {
                    // Attributes follow until the first VERTEX.
                    let mut layer = String::new();
                    let mut closed = false;
                    while i + 1 < lines.len() && lines[i].trim() != "0" {
                        let c = lines[i].trim();
                        let v = lines[i + 1];
                        i += 2;
                        if c == "8" {
                            layer = v.to_string();
                        } else if c == "70" {
                            closed = v.trim() == "1";
                        }
                    }
                    let target = match layer.as_str() {
                        DxfLayer::CUT_NAME => &mut content.cut,
                        DxfLayer::REGISTRATION_NAME => &mut content.registration,
                        other => return Err(format!("unexpected DXF layer {other:?}")),
                    };
                    let mut points = Vec::new();
                    while i + 1 < lines.len() {
                        if lines[i].trim() == "0" && lines[i + 1] == "VERTEX" {
                            i += 2;
                            let mut x = 0.0;
                            let mut y = 0.0;
                            while i + 1 < lines.len() && lines[i].trim() != "0" {
                                let c = lines[i].trim();
                                let v = lines[i + 1];
                                i += 2;
                                if c == "10" {
                                    x = v.trim().parse().map_err(|_| format!("bad x {v:?}"))?;
                                } else if c == "20" {
                                    y = v.trim().parse().map_err(|_| format!("bad y {v:?}"))?;
                                }
                            }
                            points.push(mask_kernel::Point { x, y });
                        } else if lines[i].trim() == "0" && lines[i + 1] == "SEQEND" {
                            i += 2;
                            break;
                        } else {
                            break;
                        }
                    }
                    if points.len() >= 2 {
                        target.shapes.push(Shape::new(Contour {
                            points,
                            closed,
                            arcs: Vec::new(),
                        }));
                    }
                }
                "ENDSEC" => mode = Mode::Idle,
                _ => {}
            },
            "2" if value == "ENTITIES" => mode = Mode::Idle,
            "70" if mode == Mode::InCut => {}
            _ => {
                if matches!(code, "8") {
                    // Layer assignment outside an entity context.
                    mode = match value {
                        DxfLayer::CUT_NAME => Mode::InCut,
                        DxfLayer::REGISTRATION_NAME => Mode::InRegistration,
                        _ => Mode::Idle,
                    };
                }
            }
        }
    }

    if content.cut.is_empty() && content.registration.is_empty() {
        return Err("no geometry found in DXF".to_string());
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_kernel::Point;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn content() -> DxfContent {
        DxfContent {
            cut: Shapes::single(Shape::new(Contour {
                points: vec![p(0.0, 0.0), p(100.0, 0.0), p(100.0, 50.0), p(0.0, 50.0)],
                closed: true,
                arcs: Vec::new(),
            })),
            registration: Shapes::single(Shape::new(Contour {
                points: vec![p(10.0, 10.0), p(90.0, 10.0), p(90.0, 40.0)],
                closed: true,
                arcs: Vec::new(),
            })),
        }
    }

    #[test]
    fn the_file_declares_r12_and_millimetres() {
        let dxf = to_dxf(&content(), 0.08);
        assert!(dxf.contains("AC1009"), "must be R12");
        // Without $INSUNITS a CAD app applies its own unit and the mask arrives
        // scaled by an arbitrary factor.
        assert!(dxf.contains("$INSUNITS"), "unit declaration missing");
        let idx = dxf.find("$INSUNITS").expect("$INSUNITS");
        assert!(dxf[idx..].contains("\n4\r\n"), "$INSUNITS must be 4 (mm)");
    }

    #[test]
    fn entities_use_polylines_not_lwpolylines() {
        let dxf = to_dxf(&content(), 0.08);
        assert!(dxf.contains("\nPOLYLINE\r\n"));
        assert!(!dxf.contains("LWPOLYLINE"), "R12 has no LWPOLYLINE");
    }

    #[test]
    fn line_endings_are_crlf_as_r12_requires() {
        let dxf = to_dxf(&content(), 0.08);
        assert!(dxf.contains("\r\n"));
        assert!(
            !dxf.replace("\r\n", "").contains('\n'),
            "every newline must be a CRLF"
        );
    }

    #[test]
    fn cut_and_registration_go_to_separate_layers() {
        let dxf = to_dxf(&content(), 0.08);
        assert!(dxf.contains("CUT"));
        assert!(dxf.contains("REGISTRATION"));
        // Both must also be declared in the layer table.
        assert!(dxf.contains("\nLAYER\r\n"));
    }

    #[test]
    fn all_numbers_use_the_canonical_precision() {
        let dxf = to_dxf(&content(), 0.08);
        for number in numeric_literals(&dxf) {
            let decimals = number.split('.').nth(1).map_or(0, str::len);
            assert!(
                decimals <= CANON_COORD_DP,
                "number {number} has {decimals} decimals"
            );
        }
    }

    #[test]
    fn export_is_byte_identical_across_runs() {
        assert_eq!(to_dxf(&content(), 0.08), to_dxf(&content(), 0.08));
    }

    #[test]
    fn round_trip_preserves_geometry() {
        let original = content();
        let read = from_dxf(&to_dxf(&original, 0.08)).expect("read back");
        assert_eq!(read.cut.shapes.len(), original.cut.shapes.len());
        assert_eq!(
            read.registration.shapes.len(),
            original.registration.shapes.len()
        );

        for (a, b) in original.cut.shapes.iter().zip(read.cut.shapes.iter()) {
            assert_eq!(a.outer.closed, b.outer.closed);
            assert_eq!(a.outer.points.len(), b.outer.points.len());
            for (pa, pb) in a.outer.points.iter().zip(b.outer.points.iter()) {
                assert!((pa.x - pb.x).abs() < 1e-9);
                assert!((pa.y - pb.y).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn round_trip_preserves_area() {
        let original = content();
        let read = from_dxf(&to_dxf(&original, 0.08)).expect("read back");
        assert!(
            (mask_kernel::shapes_area(&original.cut) - mask_kernel::shapes_area(&read.cut)).abs()
                < 1e-6
        );
    }

    #[test]
    fn a_tolerance_that_merges_vertices_is_respected() {
        // Two points closer than the tolerance collapse; the written vertex count
        // must reflect that rather than advertising points the knife will not
        // separate.
        let tight = DxfContent {
            cut: Shapes::single(Shape::new(Contour {
                points: vec![p(0.0, 0.0), p(0.001, 0.0), p(100.0, 0.0), p(100.0, 50.0)],
                closed: true,
                arcs: Vec::new(),
            })),
            registration: Shapes::default(),
        };
        let coarse = to_dxf(&tight, 0.5);
        let fine = to_dxf(&tight, 1e-6);
        let count = |s: &str| s.matches("\nVERTEX\r\n").count();
        assert!(
            count(&coarse) < count(&fine),
            "coarse tolerance must drop vertices"
        );
    }

    #[test]
    fn a_malformed_file_is_rejected_rather_than_partially_read() {
        let err = from_dxf("not a dxf at all").unwrap_err();
        assert!(err.contains("no geometry"), "{err}");
    }

    /// A function called `flatten` that does not flatten.
    ///
    /// This one walked `contour.points` and dropped duplicates — no arcs, no
    /// tessellation. A circle of four quarter arcs came out as four vertices: the
    /// inscribed square, in a production file whose header said curves were
    /// flattened at the plotter tolerance. Both halves were wrong, and the
    /// documentation made the second half look correct.
    ///
    /// The vertex count is the whole assertion: four is the chords, more than four
    /// is the curve.
    #[test]
    fn a_circle_reaches_the_file_as_a_curve_and_not_as_its_chords() {
        use mask_kernel::geom::{ArcSegment, Point, Shape};
        let r = 20.0;
        let points: Vec<Point> = (0..4)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * (f64::from(i));
                Point {
                    x: r * a.cos(),
                    y: r * a.sin(),
                }
            })
            .collect();
        let arcs = (0..4)
            .map(|i| ArcSegment {
                from: i,
                to: (i + 1) % 4,
                centre: Point { x: 0.0, y: 0.0 },
                radius: r,
                ccw: true,
            })
            .collect();
        let circle = Shapes::single(Shape::new(Contour {
            points,
            closed: true,
            arcs,
        }));
        let dxf = to_dxf(
            &DxfContent {
                cut: circle,
                registration: Shapes::new(),
            },
            0.08,
        );

        let vertices = dxf.matches("\nVERTEX\r\n").count();
        assert!(
            vertices > 20,
            "a circle flattened at 0.08 mm needs dozens of vertices, found {vertices}"
        );
        assert!(
            !dxf.contains("\nLWPOLYLINE\r\n"),
            "R12 has no LWPOLYLINE, whatever it costs us"
        );
    }

    /// A DXF whose curves were tessellated has to say so.
    ///
    /// R12 has no ARC, so this writer flattens at the blade's tolerance and has
    /// always said so in its own documentation — which is not the same as saying it
    /// in the file. A reader opening the DXF sees points, and nothing in the file
    /// distinguishes points that were a circle from points that were a polygon.
    #[test]
    fn the_file_declares_the_curve_tolerance_it_used() {
        let dxf = to_dxf(&content(), 0.08);
        assert!(
            dxf.contains("curves flattened at 0.0800mm chord tolerance"),
            "the tolerance must be in the file:\n{}",
            dxf.lines().take(8).collect::<Vec<_>>().join("\n")
        );
    }

    #[test]
    fn an_empty_contour_is_not_written() {
        let empty = DxfContent {
            cut: Shapes::default(),
            registration: Shapes::default(),
        };
        let dxf = to_dxf(&empty, 0.08);
        assert!(!dxf.contains("\nPOLYLINE\r\n"));
        assert!(dxf.contains("EOF"));
    }
}
