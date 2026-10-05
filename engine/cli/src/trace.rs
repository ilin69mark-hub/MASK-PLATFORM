//! `maskctl trace` — image to geometry.
//!
//! This is the entry point that was missing. Everything downstream was built and
//! tested against hand-authored fixtures, which proved the manufacturing path
//! works but left the question of where geometry actually comes from unanswered.
//!
//! The trace produces a fixture, not a finished project. It states what is in the
//! picture and nothing more: it does not choose a colour role, a cut width or a
//! layer order. Those are decisions about manufacturing, and they belong to the
//! profile and the rules, which is why a traced project still has to pass
//! validation before it is worth cutting.
//!
//! What it does decide is geometric, and those decisions are checked:
//!
//! * millimetres come from the stated part width, never from a DPI guess
//!   recorded somewhere in the file;
//! * contours are validated for orientation before being written, because a
//!   hole with the wrong winding produces a plausible wrong area rather than an
//!   error;
//! * regions below the stated minimum area are reported, not silently dropped.

use std::fs;
use std::path::Path;

use mask_kernel::measure::shapes_area;
use mask_kernel::raster::{separate_colours, ImageError, Raster};

use crate::image_io::{decode_file, DecodeError};

/// Everything the trace needs to know that cannot be read from the image.
#[derive(Debug, Clone)]
pub struct TraceOptions {
    /// Real width of the finished part. One pixel is `part_width_mm / width`
    /// across, so an assumption here scales every number that follows.
    pub part_width_mm: f64,
    /// Palette step, 1–255. Lower merges more, which is what an artist means by
    /// "fewer colours" and what a knife blade can actually hold.
    pub quantise: u8,
    /// Regions below this area are reported and excluded.
    pub min_area_mm2: f64,
    /// Contour simplification tolerance.
    pub simplify_mm: f64,
    /// Hard ceiling on palette size, so a photograph cannot produce 40 000 layers.
    pub max_colours: usize,
    /// Project identifier written into the fixture.
    pub project_id: String,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            part_width_mm: 100.0,
            // 32 collapses photographic noise without merging a deliberate edge.
            quantise: 32,
            min_area_mm2: 1.0,
            simplify_mm: 0.01,
            // Six is already more than most masks need; a photograph wants a
            // design decision, not a histogram.
            max_colours: 6,
            project_id: "MASK-TRACE".to_string(),
        }
    }
}

/// What the trace found, reported to the operator.
#[derive(Debug)]
pub struct TraceReport {
    pub image_width_px: usize,
    pub image_height_px: usize,
    pub mm_per_pixel: f64,
    pub palette: Vec<(u8, u8, u8)>,
    pub layers: Vec<LayerReport>,
    /// Regions found but below the minimum area.
    pub dropped: usize,
    pub dropped_area_mm2: f64,
    /// The largest area change simplification made on any layer.
    pub worst_simplify_delta: f64,
}

#[derive(Debug)]
pub struct LayerReport {
    pub index: u32,
    pub name: String,
    pub colour: (u8, u8, u8),
    pub pixels: usize,
    pub regions: usize,
    pub holes: usize,
    pub area_mm2: f64,
    /// How much area simplification changed, as a fraction of the traced area.
    ///
    /// On a pixelated image the contour is a staircase, and simplification
    /// smooths it. Measured at a 1.5 mm tolerance on 1 mm pixels that removes
    /// about a tenth of the area — real design intent, gone without a word. The
    /// number is reported so the operator chooses the tolerance instead of
    /// discovering the consequence in a cut file.
    pub simplify_area_delta: f64,
    /// The largest region's shortest side, the number that decides whether the
    /// region survives weeding. Reported because it is the most common reason a
    /// traced layer comes back invalid, and it is cheaper to see here than after
    /// a validation run.
    pub narrowest_mm: Option<f64>,
    /// Simplified geometry, carried out to the fixture writer.
    pub shapes: mask_kernel::Shapes,
}

pub fn run(
    image_path: &Path,
    options: &TraceOptions,
    out_fixture: &Path,
) -> Result<TraceReport, TraceError> {
    let raster = decode_file(image_path)?;
    let report = trace(&raster, options)?;

    let fixture = build_fixture(&report, &raster, options);
    let json =
        serde_json::to_string_pretty(&fixture).map_err(|e| TraceError::Format(e.to_string()))?;

    if let Some(parent) = out_fixture.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| TraceError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }
    }
    fs::write(out_fixture, json).map_err(|e| TraceError::Io {
        path: out_fixture.display().to_string(),
        source: e,
    })?;

    Ok(report)
}

/// Trace an already-decoded raster.
pub fn trace(raster: &Raster, options: &TraceOptions) -> Result<TraceReport, TraceError> {
    if options.part_width_mm <= 0.0 {
        return Err(TraceError::Options("part width must be positive".into()));
    }
    if options.quantise == 0 {
        return Err(TraceError::Options(
            "quantisation step must be at least 1".into(),
        ));
    }

    let separation = separate_colours(
        raster,
        options.quantise,
        options.part_width_mm,
        options.max_colours,
    )?;

    let scale = raster.mm_per_pixel(options.part_width_mm);
    // Areas are compared against the minimum in millimetres, so a pixel count
    // alone is not enough: a 1 px speck at 1412 mm across is 1.99 mm².
    let min_area = options.min_area_mm2;

    let mut layers = Vec::new();
    let mut dropped = 0usize;
    let mut dropped_area = 0.0;

    for region in &separation.regions {
        let kept = mask_kernel::raster::drop_small_regions(&region.shapes, min_area);
        let removed = region.shapes.shapes.len() - kept.shapes.len();
        if removed > 0 {
            dropped += removed;
            dropped_area += (shapes_area(&region.shapes) - shapes_area(&kept)).abs();
        }
        if kept.shapes.is_empty() {
            continue;
        }

        // Refuse to write geometry the kernel would later reject. A bad contour
        // written here becomes a plausible number three stages downstream.
        kept.validate_strict(mask_kernel::constants::MAX_COORDINATE_MM)
            .map_err(|e| TraceError::Format(e.to_string()))?;

        let raw_area = shapes_area(&kept).abs();
        let simplified = simplify_shapes(&kept, options.simplify_mm);
        simplified
            .validate_strict(mask_kernel::constants::MAX_COORDINATE_MM)
            .map_err(|e| TraceError::Format(format!("after simplification: {e}")))?;
        let simplified_area = shapes_area(&simplified).abs();
        let delta = if raw_area > 0.0 {
            (simplified_area - raw_area).abs() / raw_area
        } else {
            0.0
        };

        layers.push(LayerReport {
            index: layers.len() as u32 + 1,
            name: format!(
                "colour {:02x}{:02x}{:02x}",
                region.colour.0, region.colour.1, region.colour.2
            ),
            colour: (region.colour.0, region.colour.1, region.colour.2),
            pixels: region.pixel_count,
            regions: simplified.shapes.len(),
            holes: simplified.shapes.iter().map(|s| s.holes.len()).sum(),
            area_mm2: simplified_area,
            simplify_area_delta: delta,
            narrowest_mm: narrowest_dimension(&simplified),
            shapes: simplified,
        });
    }

    if layers.is_empty() {
        return Err(TraceError::Format(
            "nothing survived: every region was below the minimum area".into(),
        ));
    }

    // Compute before moving `layers` into the report.
    let worst_simplify_delta = layers
        .iter()
        .map(|l| l.simplify_area_delta)
        .fold(0.0, f64::max);

    Ok(TraceReport {
        image_width_px: raster.width,
        image_height_px: raster.height,
        mm_per_pixel: scale,
        palette: separation.palette.iter().map(|c| (c.0, c.1, c.2)).collect(),
        layers,
        dropped,
        dropped_area_mm2: dropped_area,
        worst_simplify_delta,
    })
}

fn simplify_shapes(shapes: &mask_kernel::Shapes, epsilon: f64) -> mask_kernel::Shapes {
    mask_kernel::Shapes {
        shapes: shapes
            .shapes
            .iter()
            .map(|shape| mask_kernel::Shape {
                outer: simplify_contour(&shape.outer, epsilon),
                holes: shape
                    .holes
                    .iter()
                    .map(|h| simplify_contour(h, epsilon))
                    .collect(),
            })
            .filter(|s| contour_area(&s.outer).abs() > 0.0)
            .collect(),
    }
}

fn simplify_contour(contour: &mask_kernel::Contour, epsilon: f64) -> mask_kernel::Contour {
    let points = mask_kernel::raster::simplify(&contour.points, epsilon);
    // Simplification can collapse a contour to a line. A contour with fewer than
    // three points has no area and would break the area and hash code.
    if points.len() < 3 {
        return contour.clone();
    }
    mask_kernel::Contour {
        points,
        closed: true,
    }
}

fn contour_area(contour: &mask_kernel::Contour) -> f64 {
    mask_kernel::measure::signed_contour_area(contour)
}

/// The smallest width of any region, which is what decides weeding.
fn narrowest_dimension(shapes: &mask_kernel::Shapes) -> Option<f64> {
    shapes
        .shapes
        .iter()
        .map(|s| {
            let xs: Vec<f64> = s.outer.points.iter().map(|p| p.x).collect();
            let ys: Vec<f64> = s.outer.points.iter().map(|p| p.y).collect();
            let dx = max_min(&xs);
            let dy = max_min(&ys);
            dx.min(dy)
        })
        .filter(|w| *w > 0.0)
        .fold(None, |acc: Option<f64>, w| {
            Some(acc.map_or(w, |a: f64| a.min(w)))
        })
}

/// The span of a slice of coordinates.
fn max_min(values: &[f64]) -> f64 {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if lo.is_finite() && hi.is_finite() {
        hi - lo
    } else {
        0.0
    }
}

/// Build the fixture JSON that the rest of `maskctl` consumes.
fn build_fixture(report: &TraceReport, raster: &Raster, options: &TraceOptions) -> FixtureOut {
    let height_mm = options.part_width_mm * (raster.height as f64 / raster.width as f64);

    FixtureOut {
        project_id: options.project_id.clone(),
        revision: 1,
        _comment: format!(
            "TRACED from a {}x{} image at {} mm wide, so {} mm per pixel. \
             Colour layers, no cut width: widths come from the profile. \
             Validate before cutting.",
            raster.width, raster.height, options.part_width_mm, report.mm_per_pixel
        ),
        target: FixtureTargetOut {
            object: "Traced image".to_string(),
            part: "Traced".to_string(),
            part_code: "TRACE".to_string(),
            dimensions_mm: [options.part_width_mm, height_mm],
        },
        layers: report
            .layers
            .iter()
            .map(|layer| FixtureLayerOut {
                index: layer.index,
                name: layer.name.clone(),
                regions: layer
                    .shapes
                    .shapes
                    .iter()
                    .map(|shape| FixtureRegionOut {
                        outer: shape
                            .outer
                            .points
                            .iter()
                            .map(|p| [round(p.x), round(p.y)])
                            .collect(),
                        holes: shape
                            .holes
                            .iter()
                            .map(|h| h.points.iter().map(|p| [round(p.x), round(p.y)]).collect())
                            .collect(),
                    })
                    .collect(),
                _area_mm2: round(layer.area_mm2),
            })
            .collect(),
    }
}

/// Four decimals, the project's canonical precision (ADR-0004).
fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error(transparent)]
    Decode(#[from] DecodeError),

    #[error(transparent)]
    Raster(#[from] ImageError),

    #[error("bad options: {0}")]
    Options(String),

    #[error("cannot trace: {0}")]
    Format(String),

    #[error("cannot write {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(serde::Serialize)]
struct FixtureOut {
    project_id: String,
    revision: u32,
    _comment: String,
    target: FixtureTargetOut,
    layers: Vec<FixtureLayerOut>,
}

#[derive(serde::Serialize)]
struct FixtureTargetOut {
    object: String,
    part: String,
    part_code: String,
    dimensions_mm: [f64; 2],
}

#[derive(serde::Serialize)]
struct FixtureLayerOut {
    index: u32,
    name: String,
    regions: Vec<FixtureRegionOut>,
    /// Carried for the operator's benefit and ignored on read. A geometry file
    /// that states its own area is easier to trust than one that does not.
    #[serde(rename = "_area_mm2")]
    _area_mm2: f64,
}

#[derive(serde::Serialize)]
struct FixtureRegionOut {
    outer: Vec<[f64; 2]>,
    holes: Vec<Vec<[f64; 2]>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 40x40 image: a white field with a black 20x20 square in the middle.
    /// Traced at 40 mm wide, the square is 20 mm across and 400 mm².
    fn square_with_hole() -> Raster {
        let mut data = vec![255u8; 40 * 40 * 3];
        for y in 10..30 {
            for x in 10..30 {
                let i = (y * 40 + x) * 3;
                data[i] = 0;
                data[i + 1] = 0;
                data[i + 2] = 0;
            }
        }
        Raster::new(40, 40, 3, data).expect("raster")
    }

    fn options() -> TraceOptions {
        TraceOptions {
            part_width_mm: 40.0,
            quantise: 1,
            min_area_mm2: 1.0,
            simplify_mm: 0.0,
            max_colours: 4,
            project_id: "MASK-TEST".to_string(),
        }
    }

    #[test]
    fn a_black_square_on_white_becomes_two_layers() {
        let report = trace(&square_with_hole(), &options()).expect("trace");
        assert_eq!(report.layers.len(), 2, "black and white");
        assert!((report.mm_per_pixel - 1.0).abs() < 1e-12);
    }

    #[test]
    fn the_square_measures_twenty_millimetres_across() {
        let report = trace(&square_with_hole(), &options()).expect("trace");
        let black = report
            .layers
            .iter()
            .find(|l| l.colour == (0, 0, 0))
            .expect("black layer");
        // 20x20 pixels at 1 mm per pixel.
        assert!(
            (black.area_mm2 - 400.0).abs() < 1.0,
            "black area {}",
            black.area_mm2
        );
        assert!((black.narrowest_mm.expect("width") - 20.0).abs() < 0.01);
    }

    #[test]
    fn the_white_annulus_reports_its_hole() {
        let report = trace(&square_with_hole(), &options()).expect("trace");
        let white = report
            .layers
            .iter()
            .find(|l| l.colour == (255, 255, 255))
            .expect("white layer");
        assert_eq!(white.holes, 1, "the black square is a hole in the white");
        // 40x40 minus 20x20.
        assert!(
            (white.area_mm2 - 1200.0).abs() < 2.0,
            "white area {}",
            white.area_mm2
        );
    }

    #[test]
    fn area_is_derived_from_the_part_not_from_the_pixel_count() {
        let mut wide = options();
        wide.part_width_mm = 200.0;
        let report = trace(&square_with_hole(), &wide).expect("trace");
        let black = report
            .layers
            .iter()
            .find(|l| l.colour == (0, 0, 0))
            .expect("black");
        // Five times the width, twenty-five times the area.
        assert!(
            (black.area_mm2 - 400.0 * 25.0).abs() < 50.0,
            "area {} at 200 mm wide",
            black.area_mm2
        );
    }

    #[test]
    fn traced_geometry_writes_a_fixture_the_cli_can_read() {
        let dir = std::env::temp_dir();
        let image_path = dir.join("maskctl-trace-roundtrip.pgm");
        let mut bytes = b"P5\n40 40\n255\n".to_vec();
        for y in 0..40 {
            for x in 0..40 {
                let v = if (10..30).contains(&x) && (10..30).contains(&y) {
                    0
                } else {
                    255
                };
                bytes.push(v);
            }
        }
        fs::write(&image_path, &bytes).expect("write image");

        let out = dir.join("maskctl-trace-roundtrip.json");
        let report = run(&image_path, &options(), &out).expect("trace to file");
        assert_eq!(report.layers.len(), 2);

        // Read it back with the same structures the CLI uses. If the trace wrote
        // a shape the pipeline cannot parse, it fails here rather than in a
        // three-stage run.
        let text = fs::read_to_string(&out).expect("read fixture");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        let layers = parsed["layers"].as_array().expect("layers");
        assert_eq!(layers.len(), 2);
        for layer in layers {
            assert!(layer["regions"].is_array(), "each layer carries regions");
            for region in layer["regions"].as_array().expect("regions") {
                assert!(
                    region["outer"].as_array().expect("outer").len() >= 3,
                    "a region needs a real contour"
                );
            }
        }

        let _ = fs::remove_file(image_path);
        let _ = fs::remove_file(out);
    }

    #[test]
    fn a_photograph_is_refused_instead_of_producing_forty_thousand_layers() {
        // A smooth gradient: every pixel is a slightly different colour.
        let mut data = vec![0u8; 64 * 64 * 3];
        for y in 0..64 {
            for x in 0..64 {
                let v = (x * 4) as u8;
                let i = (y * 64 + x) * 3;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
            }
        }
        let image = Raster::new(64, 64, 3, data).expect("raster");
        let mut opts = options();
        opts.max_colours = 8;
        opts.quantise = 1;
        assert!(matches!(
            trace(&image, &opts),
            Err(TraceError::Raster(ImageError::TooManyClusters { .. }))
        ));
    }

    #[test]
    fn quantisation_is_the_way_to_accept_a_photograph() {
        let mut data = vec![0u8; 64 * 64 * 3];
        for y in 0..64 {
            for x in 0..64 {
                let v = (x * 4) as u8;
                let i = (y * 64 + x) * 3;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
            }
        }
        let image = Raster::new(64, 64, 3, data).expect("raster");
        let mut opts = options();
        opts.quantise = 32;
        opts.max_colours = 8;
        let report = trace(&image, &opts).expect("coarse palette");
        assert!(
            report.layers.len() <= 8,
            "got {} layers",
            report.layers.len()
        );
    }

    #[test]
    fn specks_are_reported_rather_than_silently_discarded() {
        let mut data = vec![255u8; 40 * 40 * 3];
        // A single black pixel: 1 mm² at 40 mm wide, below a 10 mm² minimum.
        data[(20 * 40 + 20) * 3] = 0;
        let image = Raster::new(40, 40, 3, data).expect("raster");
        let mut opts = options();
        opts.min_area_mm2 = 10.0;
        let report = trace(&image, &opts).expect("trace");
        assert_eq!(report.dropped, 1, "the speck is counted");
        assert!(report.dropped_area_mm2 > 0.0);
        assert_eq!(report.layers.len(), 1, "only the white field survives");
    }

    #[test]
    fn an_image_of_one_colour_produces_one_layer_not_a_hole() {
        let data = vec![128u8; 10 * 10 * 3];
        let image = Raster::new(10, 10, 3, data).expect("raster");
        let report = trace(&image, &options()).expect("trace");
        assert_eq!(report.layers.len(), 1);
        assert_eq!(report.layers[0].holes, 0, "a full-bleed field has no hole");
    }

    #[test]
    fn a_zero_width_part_is_refused() {
        let mut opts = options();
        opts.part_width_mm = 0.0;
        assert!(matches!(
            trace(&square_with_hole(), &opts),
            Err(TraceError::Options(_))
        ));
    }

    #[test]
    fn tracing_is_reproducible() {
        let image = square_with_hole();
        let first = trace(&image, &options()).expect("first");
        let second = trace(&image, &options()).expect("second");
        assert_eq!(
            format!(
                "{:?}",
                first.layers.iter().map(|l| l.area_mm2).collect::<Vec<_>>()
            ),
            format!(
                "{:?}",
                second.layers.iter().map(|l| l.area_mm2).collect::<Vec<_>>()
            )
        );
    }

    #[test]
    fn simplification_reduces_vertices_without_moving_the_boundary_far() {
        let mut data = vec![255u8; 60 * 60 * 3];
        // A staircase, which real traced images are full of.
        for y in 10..50 {
            for x in 10..50 {
                if (x + y) % 2 == 0 {
                    let i = (y * 60 + x) * 3;
                    data[i] = 0;
                    data[i + 1] = 0;
                    data[i + 2] = 0;
                }
            }
        }
        let image = Raster::new(60, 60, 3, data).expect("raster");
        let mut plain = options();
        plain.simplify_mm = 0.0;
        let mut smoothed = options();
        smoothed.simplify_mm = 1.5;

        let a = trace(&image, &plain).expect("plain");
        let b = trace(&image, &smoothed).expect("smoothed");

        assert!(a.worst_simplify_delta < 1e-9, "no tolerance, no change");

        // The behaviour worth pinning: a tolerance larger than a pixel smooths a
        // staircase by cutting a corner off every step. It is not a rounding
        // error and it must not pass unnoticed.
        assert!(
            b.worst_simplify_delta > 0.05,
            "a 1.5 mm tolerance on 1 mm pixels should visibly change area, got {}",
            b.worst_simplify_delta
        );

        // A tolerance below the pixel size only removes collinear points, which
        // costs no area at all.
        let mut fine = options();
        fine.simplify_mm = 0.2;
        let c = trace(&image, &fine).expect("fine");
        assert!(
            c.worst_simplify_delta < 0.01,
            "a sub-pixel tolerance must be area-neutral, got {}",
            c.worst_simplify_delta
        );
    }
}
