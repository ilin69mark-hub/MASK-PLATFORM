//! Export. Derivative artifacts only.
//!
//! Nothing here may modify the project (P-02). Export reads a validated
//! project and writes files; if a file looks wrong, the project is wrong, and
//! the fix belongs upstream.
//!
//! Determinism (ADR-0004 D2): the same project revision and the same engine
//! version must produce byte-identical output. No clock, no locale, no hash-map
//! iteration order, fixed decimal precision.

pub mod dxf;
pub mod manifest;
pub mod plt;

use mask_kernel::{Shape, Shapes, CANON_COORD_DP};
use mask_nesting::RegistrationMarks;
use mask_validate::{ValidationReport, Verdict};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("refusing to export: verdict is {verdict:?}")]
    NotManufacturable { verdict: Verdict },

    #[error("layer {index} has no geometry")]
    EmptyLayer { index: u32 },
}

/// Registration marks to draw alongside a tile.
#[derive(Debug, Clone, PartialEq)]
pub struct MarksToDraw {
    pub marks: RegistrationMarks,
    /// Tile outline so the worker can see the printable area.
    pub rect: [f64; 4],
}

/// Identity written into every artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactContext {
    pub project_id: String,
    pub revision: u32,
    pub engine_version: String,
    pub kerf_mm: f64,
}

/// The window an SVG shows, in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub min_x: f64,
    pub min_y: f64,
    pub width: f64,
    pub height: f64,
}

impl View {
    /// Default view: exactly the geometry's bounding box.
    #[must_use]
    pub fn of(geometry: &Shapes) -> Self {
        let (min_x, min_y, w, h) = mask_kernel::bounding_box(geometry);
        Self {
            min_x,
            min_y,
            width: w,
            height: h,
        }
    }
}

/// Write one layer as an SVG in millimetres.
///
/// Axis convention: Y up, origin bottom-left, matching the plotter profile.
/// This is fixed by ADR-0009 and must not drift.
#[must_use]
pub fn to_svg(
    geometry: &Shapes,
    layer_index: u32,
    layer_name: &str,
    report: &ValidationReport,
    ctx: &ArtifactContext,
) -> String {
    to_svg_with_view(
        geometry,
        View::of(geometry),
        layer_index,
        layer_name,
        report,
        ctx,
    )
}

/// Write one layer as an SVG showing an explicit window.
///
/// A tile needs this: its registration marks and printable-area outline live
/// outside the cut geometry, so a view derived from the content alone would
/// clip them away and leave the worker with nothing to align against.
#[must_use]
pub fn to_svg_with_view(
    geometry: &Shapes,
    view: View,
    layer_index: u32,
    layer_name: &str,
    report: &ValidationReport,
    ctx: &ArtifactContext,
) -> String {
    let kerf = ctx.kerf_mm;
    let mut s = String::with_capacity(4096 + geometry.shapes.len() * 256);

    // Header numbers go through the same quantisation as path data. Printing raw
    // f64 here produced `150.84999990463257mm`, which is deterministic but is not
    // the canonical form ADR-0004 requires, and it contradicted the 4-decimal
    // path data in the same file.
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"\n",
            "     version=\"1.1\"\n",
            "     width=\"{}mm\" height=\"{}mm\"\n",
            "     viewBox=\"{} {} {} {}\">\n"
        ),
        num(view.width),
        num(view.height),
        num(view.min_x),
        num(view.min_y),
        num(view.width),
        num(view.height),
    ));

    // Metadata travels with the file. A mask found on a bench years later must
    // be identifiable without the database.
    s.push_str("  <metadata>\n    <maskproject>\n");
    let revision = ctx.revision.to_string();
    let layer_no = layer_index.to_string();
    let kerf_label = format!("{kerf}mm");
    let verdict_label = format!("{:?}", report.verdict);
    let verified_label = report.limits_physically_verified.to_string();
    for (key, value) in [
        ("id", ctx.project_id.as_str()),
        ("revision", revision.as_str()),
        ("layer", layer_no.as_str()),
        ("layer_name", layer_name),
        ("engine", ctx.engine_version.as_str()),
        ("kerf_mm", kerf_label.as_str()),
        ("verdict", verdict_label.as_str()),
        ("limits_verified", verified_label.as_str()),
    ] {
        s.push_str(&format!("      <{key}>{value}</{key}>\n"));
    }
    s.push_str("    </maskproject>\n  </metadata>\n");

    // Cut geometry: stroke width equals the kerf so the line reads as the
    // material the knife removes.
    s.push_str(&format!(
        "  <g id=\"cut\" fill=\"none\" stroke=\"#000000\" stroke-width=\"{kerf}\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\n"
    ));
    for (index, shape) in geometry.shapes.iter().enumerate() {
        emit_contour(&mut s, &shape.outer, index);
        for hole in &shape.holes {
            emit_contour(&mut s, hole, index);
        }
    }
    s.push_str("  </g>\n</svg>\n");
    s
}

fn emit_contour(out: &mut String, contour: &mask_kernel::Contour, region: usize) {
    if contour.points.is_empty() {
        return;
    }
    out.push_str(&format!(
        "    <path id=\"path_r{region}\" d=\"{}\"/>\n",
        mask_kernel::to_path_data(contour)
    ));
}

/// Write the whole project: one SVG per layer.
pub fn project_svgs(
    layers: &[(u32, String, Shapes)],
    report: &ValidationReport,
    ctx: &ArtifactContext,
) -> Result<Vec<(u32, String, String)>, ExportError> {
    if !report.verdict.is_manufacturable() {
        return Err(ExportError::NotManufacturable {
            verdict: report.verdict,
        });
    }
    let mut out = Vec::with_capacity(layers.len());
    for (index, name, geometry) in layers {
        if geometry.is_empty() {
            return Err(ExportError::EmptyLayer { index: *index });
        }
        out.push((
            *index,
            name.clone(),
            to_svg(geometry, *index, name, report, ctx),
        ));
    }
    Ok(out)
}

/// Write one tile of a nested layer, with its registration marks.
///
/// The marks are what make reassembly possible. A tile file without them is not
/// a cuttable artifact: the worker would have no way to align the piece with the
/// part, or with its neighbour, and would be guessing.
#[must_use]
pub fn tile_to_svg(
    geometry: &Shapes,
    layer_index: u32,
    layer_name: &str,
    tile_id: &str,
    tile: &MarksToDraw,
    report: &ValidationReport,
    ctx: &ArtifactContext,
) -> String {
    // The view must cover the whole tile, not just the cut geometry: the marks
    // and the printable-area outline sit in the margin, and clipping them away
    // would leave the worker with no alignment reference at all.
    let [rx, ry, rw, rh] = tile.rect;
    let view = View {
        min_x: rx,
        min_y: ry,
        width: rw.max(0.001),
        height: rh.max(0.001),
    };
    let mut svg = to_svg_with_view(geometry, view, layer_index, layer_name, report, ctx);

    let mut body = String::from(
        "\n  <g id=\"registration\" fill=\"none\" stroke=\"#000000\" stroke-width=\"0.1\">\n",
    );

    let cross = |x: f64, y: f64| {
        format!(
            concat!("    <path d=\"M {x0} {y0} L {x0} {y1} M {x1} {y0} L {x0} {y0}\"/>\n"),
            x0 = num(x),
            y0 = num(y),
            x1 = num(x + 3.0),
            y1 = num(y + 3.0),
        )
    };

    let pa = &tile.marks.part_anchor;
    let ta = &tile.marks.tile_anchor;
    body.push_str(&cross(pa.left.x_mm, pa.left.y_mm));
    body.push_str(&cross(pa.right.x_mm, pa.right.y_mm));
    body.push_str(&cross(ta.left.x_mm, ta.left.y_mm));
    body.push_str(&cross(ta.right.x_mm, ta.right.y_mm));

    let [rx, ry, rw, rh] = tile.rect;
    body.push_str(&format!(
        "    <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" stroke-dasharray=\"4 2\"/>\n",
        num(rx),
        num(ry),
        num(rw),
        num(rh)
    ));

    // Printed large: the label is read by eye at the bench, not zoomed.
    body.push_str(&format!(
        "    <text x=\"{}\" y=\"{}\" fill=\"#000000\" stroke=\"none\" font-size=\"8\">{tile_id}</text>\n",
        num(rx + rw / 2.0),
        num(ry + rh - 4.0)
    ));

    if tile.marks.orientation {
        let ax = rx + rw / 2.0;
        let ay = ry + 4.0;
        body.push_str(&format!(
            "    <path d=\"M {} {} L {} {} M {} {} L {} {}\"/>\n",
            num(ax),
            num(ay + 6.0),
            num(ax),
            num(ay),
            num(ax + 2.5),
            num(ay + 2.5),
            num(ax),
            num(ay)
        ));
    }

    body.push_str("  </g>\n</svg>\n");

    svg.truncate(svg.len() - "</svg>\n".len());
    svg.push_str(&body);
    svg
}

/// Format a coordinate at the canonical precision.
fn num(value: f64) -> String {
    format!("{value:.prec$}", prec = CANON_COORD_DP)
}

/// File name for a layer artifact.
///
/// Encodes project, revision and layer so a directory of masks is
/// self-describing (EXPORT_ENGINE.md §9).
#[must_use]
pub fn artifact_name(project_id: &str, revision: u32, layer_index: u32, extension: &str) -> String {
    format!(
        "{}.{extension}",
        layer_stem(project_id, revision, layer_index)
    )
}

/// Every numeric literal in a document, so precision can be asserted.
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
            // Skip version-like and identifier-adjacent tokens.
            if token.contains('.') {
                out.push(token);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// File name for one tile of a nested layer.
///
/// A tile file names both the layer and the tile, because a bench worker
/// receives sheets and needs to know which piece of which layer they hold.
#[must_use]
pub fn tile_artifact_name(
    project_id: &str,
    revision: u32,
    layer_index: u32,
    tile_id: &str,
    extension: &str,
) -> String {
    format!(
        "{}.{extension}",
        tile_stem(project_id, revision, layer_index, tile_id)
    )
}

/// File-name stem for a whole layer.
#[must_use]
pub fn layer_stem(project_id: &str, revision: u32, layer_index: u32) -> String {
    format!("{project_id}_rev{revision}_L{layer_index:02}")
}

/// File-name stem for one tile of a layer.
#[must_use]
pub fn tile_stem(project_id: &str, revision: u32, layer_index: u32, tile_id: &str) -> String {
    format!("{project_id}_rev{revision}_L{layer_index:02}_{tile_id}")
}

/// Hex SHA-256 of some bytes.
///
/// Used for artifact rows in the manifest. The kernel owns geometry hashing;
/// this exists so the manifest can record file digests from the same crate that
/// writes the files.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Decimal places used for coordinates, exposed for tests.
#[must_use]
pub fn coordinate_precision() -> usize {
    CANON_COORD_DP
}

/// Total area of a shape, re-exported for convenience in downstream crates.
#[must_use]
pub fn area_of(shape: &Shape) -> f64 {
    mask_kernel::shape_area(shape)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_kernel::{Contour, Point};
    use mask_validate::Counts;

    fn ctx() -> ArtifactContext {
        ArtifactContext {
            project_id: "MASK-000127".into(),
            revision: 4,
            engine_version: "0.1.0".into(),
            kerf_mm: 0.15,
        }
    }

    fn passing_report() -> ValidationReport {
        ValidationReport {
            verdict: Verdict::Pass,
            counts: Counts {
                fatal: 0,
                error: 0,
                warning: 0,
                info: 0,
            },
            violations: vec![],
            layers: vec![],
            fidelity: Some(0.91),
            limits_physically_verified: false,
        }
    }

    fn rect(w: f64, h: f64) -> Shapes {
        Shapes::single(Shape::new(Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: w, y: 0.0 },
                Point { x: w, y: h },
                Point { x: 0.0, y: h },
            ],
            closed: true,
            arcs: Vec::new(),
        }))
    }

    #[test]
    fn svg_declares_millimetres_and_an_upward_axis() {
        let svg = to_svg(&rect(100.0, 50.0), 1, "Base", &passing_report(), &ctx());
        assert!(svg.contains("width=\"100.0000mm\""));
        assert!(svg.contains("height=\"50.0000mm\""));
        // viewBox origin equals the geometry bounds, not a flipped one.
        assert!(svg.contains("viewBox=\"0.0000 0.0000 100.0000 50.0000\""));
    }

    #[test]
    fn svg_embeds_project_identity() {
        let svg = to_svg(&rect(10.0, 10.0), 3, "Red Base", &passing_report(), &ctx());
        assert!(svg.contains("<id>MASK-000127</id>"));
        assert!(svg.contains("<revision>4</revision>"));
        assert!(svg.contains("<layer>3</layer>"));
        assert!(svg.contains("<engine>0.1.0</engine>"));
    }

    #[test]
    fn svg_records_that_limits_are_unverified() {
        // A bench worker must be able to tell a calculated pass from a proven one.
        let svg = to_svg(&rect(10.0, 10.0), 1, "Base", &passing_report(), &ctx());
        assert!(svg.contains("<limits_verified>false</limits_verified>"));
    }

    #[test]
    fn svg_stroke_width_equals_the_kerf() {
        let svg = to_svg(&rect(10.0, 10.0), 1, "Base", &passing_report(), &ctx());
        assert!(svg.contains("stroke-width=\"0.15\""), "{svg}");
    }

    #[test]
    fn svg_header_uses_the_same_precision_as_path_data() {
        // Regression: the header once printed raw f64 (`150.84999990463257mm`)
        // while path data was at 4 decimals, so one file carried two
        // precisions. Determinism held, but the canonical form did not.
        let svg = to_svg(&rect(10.0, 5.0), 1, "Base", &passing_report(), &ctx());
        assert!(svg.contains("width=\"10.0000mm\""), "{svg}");
        assert!(svg.contains("height=\"5.0000mm\""), "{svg}");
        for number in numeric_literals(&svg) {
            let decimals = number.split('.').nth(1).map_or(0, str::len);
            assert!(
                decimals <= CANON_COORD_DP,
                "number {number} has {decimals} decimals, more than {CANON_COORD_DP}: {svg}"
            );
        }
    }

    #[test]
    fn svg_uses_fixed_coordinate_precision() {
        let svg = to_svg(&rect(10.0, 5.0), 1, "Base", &passing_report(), &ctx());
        assert!(svg.contains("M 0.0000 0.0000 L 10.0000 0.0000"), "{svg}");
        assert_eq!(coordinate_precision(), 4);
    }

    #[test]
    fn export_is_byte_identical_across_runs() {
        let a = to_svg(&rect(100.0, 50.0), 1, "Base", &passing_report(), &ctx());
        let b = to_svg(&rect(100.0, 50.0), 1, "Base", &passing_report(), &ctx());
        assert_eq!(a, b);
    }

    #[test]
    fn holes_are_emitted_alongside_the_outer_contour() {
        let with_hole = Shapes::single(Shape::with_holes(
            Contour {
                points: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 10.0, y: 0.0 },
                    Point { x: 10.0, y: 10.0 },
                    Point { x: 0.0, y: 10.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            },
            vec![Contour {
                points: vec![
                    Point { x: 3.0, y: 3.0 },
                    Point { x: 7.0, y: 3.0 },
                    Point { x: 7.0, y: 7.0 },
                    Point { x: 3.0, y: 7.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            }],
        ));
        let svg = to_svg(&with_hole, 1, "Base", &passing_report(), &ctx());
        assert_eq!(svg.matches("<path").count(), 2, "outer plus hole");
    }

    #[test]
    fn a_failing_project_cannot_be_exported() {
        let mut report = passing_report();
        report.verdict = Verdict::Fail;
        let err = project_svgs(&[(1, "Base".into(), rect(10.0, 10.0))], &report, &ctx());
        assert!(matches!(err, Err(ExportError::NotManufacturable { .. })));
    }

    #[test]
    fn an_empty_layer_cannot_be_exported() {
        let err = project_svgs(
            &[(1, "Base".into(), Shapes::new())],
            &passing_report(),
            &ctx(),
        );
        assert!(matches!(err, Err(ExportError::EmptyLayer { index: 1 })));
    }

    #[test]
    fn artifact_names_encode_identity() {
        assert_eq!(
            artifact_name("MASK-000127", 4, 3, "svg"),
            "MASK-000127_rev4_L03.svg"
        );
    }

    #[test]
    fn exported_area_matches_the_geometry() {
        assert!(
            (area_of(&Shape::new(Contour {
                points: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 10.0, y: 0.0 },
                    Point { x: 10.0, y: 5.0 },
                    Point { x: 0.0, y: 5.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            })) - 50.0)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn svg_is_well_formed_xml() {
        let svg = to_svg(&rect(10.0, 10.0), 1, "Base", &passing_report(), &ctx());
        assert!(svg.starts_with("<?xml"));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches('<').count(), svg.matches('>').count());
    }
}

#[cfg(test)]
mod tile_tests {
    use super::*;
    use mask_kernel::{Contour, Point};
    use mask_nesting::{MarkPoint, MarkPoints, RegistrationMarks};

    fn marks() -> RegistrationMarks {
        RegistrationMarks {
            part_anchor: MarkPoints {
                left: MarkPoint {
                    x_mm: 10.0,
                    y_mm: 50.0,
                },
                right: MarkPoint {
                    x_mm: 270.0,
                    y_mm: 50.0,
                },
            },
            tile_anchor: MarkPoints {
                left: MarkPoint {
                    x_mm: 140.0,
                    y_mm: 10.0,
                },
                right: MarkPoint {
                    x_mm: 140.0,
                    y_mm: 90.0,
                },
            },
            orientation: true,
            label: "A2".into(),
        }
    }

    fn tile() -> MarksToDraw {
        MarksToDraw {
            marks: marks(),
            rect: [0.0, 0.0, 284.0, 100.0],
        }
    }

    fn ctx() -> ArtifactContext {
        ArtifactContext {
            project_id: "MASK-000127".into(),
            revision: 2,
            engine_version: "maskctl/0.1.0".into(),
            kerf_mm: 0.15,
        }
    }

    fn report() -> ValidationReport {
        ValidationReport {
            verdict: Verdict::Pass,
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
        }
    }

    fn content() -> Shapes {
        Shapes::single(Shape::new(Contour {
            points: vec![
                Point { x: 40.0, y: 30.0 },
                Point { x: 240.0, y: 30.0 },
                Point { x: 240.0, y: 70.0 },
                Point { x: 40.0, y: 70.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        }))
    }

    #[test]
    fn a_tile_shows_the_whole_tile_not_just_the_cut() {
        // Regression: the view was derived from the content bounding box, which
        // clipped the marks and the printable-area outline away. The worker
        // would have had nothing to align against.
        let svg = tile_to_svg(&content(), 1, "Base", "A2", &tile(), &report(), &ctx());
        assert!(
            svg.contains("viewBox=\"0.0000 0.0000 284.0000 100.0000\""),
            "{svg}"
        );
        assert!(svg.contains("width=\"284.0000mm\""));
    }

    #[test]
    fn a_tile_carries_all_four_registration_crosses() {
        let svg = tile_to_svg(&content(), 1, "Base", "A2", &tile(), &report(), &ctx());
        let crosses = svg.matches("<path d=\"M 10.0000").count()
            + svg.matches("<path d=\"M 270.0000").count()
            + svg.matches("<path d=\"M 140.0000 10.0000").count()
            + svg.matches("<path d=\"M 140.0000 90.0000").count();
        assert_eq!(crosses, 4, "part anchor pair plus tile anchor pair");
    }

    #[test]
    fn a_tile_is_labelled_with_its_own_id() {
        let svg = tile_to_svg(&content(), 1, "Base", "A2", &tile(), &report(), &ctx());
        assert!(svg.contains(">A2</text>"), "tile label missing: {svg}");
    }

    #[test]
    fn the_printable_area_is_outlined() {
        let svg = tile_to_svg(&content(), 1, "Base", "A2", &tile(), &report(), &ctx());
        assert!(
            svg.contains("stroke-dasharray=\"4 2\""),
            "tile outline missing"
        );
    }

    #[test]
    fn tile_export_is_deterministic() {
        let a = tile_to_svg(&content(), 1, "Base", "A2", &tile(), &report(), &ctx());
        let b = tile_to_svg(&content(), 1, "Base", "A2", &tile(), &report(), &ctx());
        assert_eq!(a, b);
    }

    #[test]
    fn a_tile_file_names_the_layer_and_the_tile() {
        assert_eq!(
            tile_artifact_name("MASK-000127", 2, 1, "A2", "svg"),
            "MASK-000127_rev2_L01_A2.svg"
        );
    }
}
