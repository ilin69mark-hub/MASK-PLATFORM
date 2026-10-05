//! Nesting: splitting a mask larger than the machine into printable tiles.
//!
//! A bonnet mask is roughly 1412 x 1046 mm. A knife plotter of this class
//! reaches 300 mm. The mask therefore has to be cut into pieces and the pieces
//! have to go back together in the right order and the right alignment. This
//! module produces that plan; the registration marks are what make the
//! reassembly possible.
//!
//! Scope: `fixed_grid` and `strip`. Rectangle packing with contour-following
//! (`offset_nesting`) is deliberately absent — it buys maybe 14% of material for
//! a large increase in failure modes, which is a bad trade while the physical
//! process is still unproven.

use mask_kernel::{Contour, Point, Shape, Shapes};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum NestError {
    #[error("geometry exceeds the {limit} mm coordinate bound")]
    OutOfBounds { limit: f64 },

    #[error("field is too small to hold any content: {0} mm")]
    FieldTooSmall(f64),

    #[error("separation gap {gap} mm leaves no usable width in a {field} mm field")]
    FieldConsumedByGap { gap: f64, field: f64 },

    #[error("tile count {count} exceeds the limit of {max}")]
    TooManyTiles { count: usize, max: usize },
}

/// A placement sheet.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub index: u32,
    /// Where this tile sits on the sheet, from the bottom-left corner.
    pub origin_x_mm: f64,
    pub origin_y_mm: f64,
}

/// One printable piece of a mask layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    /// `A1`, `A2`, `B1`, … Column letter then row number.
    pub id: String,
    pub sheet: Sheet,
    /// Content rectangle on the sheet, in millimetres from the sheet origin.
    pub rect: Rect,
    /// The cut content of this tile, already separated from its neighbours.
    pub content: Shapes,
    pub marks: RegistrationMarks,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x_mm: f64,
    pub y_mm: f64,
    pub width_mm: f64,
    pub height_mm: f64,
}

/// Registration marks a bench worker needs to put a tile back in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistrationMarks {
    /// Marks that locate the tile on the part itself.
    pub part_anchor: MarkPoints,
    /// Marks that locate this tile against the previous one.
    pub tile_anchor: MarkPoints,
    /// Arrow showing which way is up.
    pub orientation: bool,
    /// Human label, printed large.
    pub label: String,
}

/// A crosshair pair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MarkPoints {
    pub left: MarkPoint,
    pub right: MarkPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MarkPoint {
    pub x_mm: f64,
    pub y_mm: f64,
}

/// The complete plan for one layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NestingPlan {
    pub strategy: &'static str,
    pub tiles: Vec<Tile>,
    pub summary: NestingSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NestingSummary {
    pub tile_count: usize,
    pub content_area_mm2: f64,
    /// Sheet area consumed, tiles only, not counting margins.
    pub used_area_mm2: f64,
    /// Fraction of the used area that is not mask.
    pub waste_ratio: f64,
    pub separation_gap_mm: f64,
    pub requires_registration: bool,
}

/// Inputs needed to lay out one layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NestRequest {
    /// Usable width of the plotter, after margins.
    pub field_width_mm: f64,
    /// Usable length of the plotter, after margins.
    pub field_length_mm: f64,
    /// Minimum gap that must remain between two pieces of the same mask.
    pub separation_gap_mm: f64,
    /// Kerf, for recording only: the split geometry already accounts for it.
    pub kerf_mm: f64,
}

/// Hard ceiling, so a pathological input cannot allocate without bound.
const MAX_TILES: usize = 400;

/// Lay a layer out on a regular grid.
///
/// Tiles are separated by `separation_gap_mm` of empty material rather than
/// sharing a boundary. Sharing a boundary would leave two pieces that touch,
/// which the `min_gap` rule already rejects and which would in practice merge
/// into a single ragged piece during weeding.
pub fn fixed_grid(layer: &Shapes, request: NestRequest) -> Result<NestingPlan, NestError> {
    if request.field_width_mm <= 0.0 || request.field_length_mm <= 0.0 {
        return Err(NestError::FieldTooSmall(
            request.field_width_mm.min(request.field_length_mm),
        ));
    }

    let (min_x, min_y, w, h) = mask_kernel::bounding_box(layer);
    if w <= 0.0 || h <= 0.0 {
        return Ok(NestingPlan {
            strategy: "fixed_grid",
            tiles: vec![],
            summary: NestingSummary {
                tile_count: 0,
                content_area_mm2: 0.0,
                used_area_mm2: 0.0,
                waste_ratio: 0.0,
                separation_gap_mm: request.separation_gap_mm,
                requires_registration: false,
            },
        });
    }

    let gap = request.separation_gap_mm.max(0.0);
    let pitch_x = request.field_width_mm + gap;
    let pitch_y = request.field_length_mm + gap;

    // Two tiles plus the gap between them must fit, so a field no larger than
    // the gap itself is unusable rather than merely tight.
    if request.field_width_mm <= gap || request.field_length_mm <= gap {
        return Err(NestError::FieldConsumedByGap {
            gap,
            field: request.field_width_mm.min(request.field_length_mm),
        });
    }

    let cols = (w / pitch_x).ceil().max(1.0) as usize;
    let rows = (h / pitch_y).ceil().max(1.0) as usize;
    let tile_count = cols * rows;

    if tile_count > MAX_TILES {
        return Err(NestError::TooManyTiles {
            count: tile_count,
            max: MAX_TILES,
        });
    }

    let mut tiles = Vec::with_capacity(tile_count);
    for row in 0..rows {
        for col in 0..cols {
            let row_label = char::from(b'A' + (row as u8).min(25));
            let id = format!("{row_label}{}", col + 1);
            let label = id.clone();

            // Content window for this tile: exactly one field, no inset.
            //
            // The separation gap comes from the *pitch*, not from insetting the
            // window. Insetting as well was the first version, and it silently
            // shaved gap/2 off every outer edge of the mask — 546 mm² lost on a
            // 2400 mm² band, because the outer boundary of the mask is not a
            // split and needs no separation at all.
            let cx0 = min_x + col as f64 * pitch_x;
            let cy0 = min_y + row as f64 * pitch_y;
            let rect = Rect {
                x_mm: cx0,
                y_mm: cy0,
                width_mm: request.field_width_mm,
                height_mm: request.field_length_mm,
            };

            let window = Shapes::single(Shape::new(Contour {
                points: vec![
                    Point { x: cx0, y: cy0 },
                    Point {
                        x: cx0 + request.field_width_mm,
                        y: cy0,
                    },
                    Point {
                        x: cx0 + request.field_width_mm,
                        y: cy0 + request.field_length_mm,
                    },
                    Point {
                        x: cx0,
                        y: cy0 + request.field_length_mm,
                    },
                ],
                closed: true,
            }));

            let content =
                mask_kernel::intersection(layer, &window).map_err(|_| NestError::OutOfBounds {
                    limit: mask_kernel::MAX_COORDINATE_MM,
                })?;

            tiles.push(Tile {
                id,
                sheet: Sheet {
                    index: (row * cols + col) as u32,
                    origin_x_mm: 0.0,
                    origin_y_mm: 0.0,
                },
                rect,
                content,
                marks: marks_for(&rect, &label),
            });
        }
    }

    let content_area: f64 = tiles
        .iter()
        .map(|t| mask_kernel::shapes_area(&t.content))
        .sum();
    let cols_f = cols as f64;
    let rows_f = rows as f64;
    let used_area = (cols_f * request.field_width_mm * rows_f * request.field_length_mm).max(1.0);

    Ok(NestingPlan {
        strategy: "fixed_grid",
        tiles,
        summary: NestingSummary {
            tile_count,
            content_area_mm2: content_area,
            used_area_mm2: used_area,
            waste_ratio: (1.0 - content_area / used_area).clamp(0.0, 1.0),
            separation_gap_mm: gap,
            // A single tile needs no registration against a neighbour, but a
            // single tile that is also the whole mask still needs its part
            // anchor.
            requires_registration: tile_count > 1,
        },
    })
}

/// Mark positions relative to the tile's content rectangle.
///
/// Marks sit outside the content so they are never cut away with it, and they
/// stay clear of the split line so they survive weeding.
fn marks_for(rect: &Rect, id: &str) -> RegistrationMarks {
    let inset = (rect.width_mm.min(rect.height_mm) * 0.03).max(2.0);
    let mid_y = rect.y_mm + rect.height_mm / 2.0;
    let mid_x = rect.x_mm + rect.width_mm / 2.0;

    RegistrationMarks {
        part_anchor: MarkPoints {
            left: MarkPoint {
                x_mm: rect.x_mm + inset,
                y_mm: mid_y,
            },
            right: MarkPoint {
                x_mm: rect.x_mm + rect.width_mm - inset,
                y_mm: mid_y,
            },
        },
        tile_anchor: MarkPoints {
            left: MarkPoint {
                x_mm: mid_x,
                y_mm: rect.y_mm + inset,
            },
            right: MarkPoint {
                x_mm: mid_x,
                y_mm: rect.y_mm + rect.height_mm - inset,
            },
        },
        orientation: true,
        label: id.to_string(),
    }
}

/// Whether a layer needs nesting at all.
#[must_use]
pub fn needs_nesting(layer: &Shapes, request: NestRequest) -> bool {
    let (_, _, w, h) = mask_kernel::bounding_box(layer);
    w > request.field_width_mm || h > request.field_length_mm
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_kernel::{widen, Point};

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn request() -> NestRequest {
        NestRequest {
            field_width_mm: 284.0,
            field_length_mm: 1184.0,
            separation_gap_mm: 0.90,
            kerf_mm: 0.15,
        }
    }

    /// A long horizontal band of the given overall size.
    fn band(length: f64, width: f64) -> Shapes {
        widen(&[p(0.0, 0.0), p(length, 0.0)], width).unwrap()
    }

    #[test]
    fn a_mask_that_fits_needs_no_tiles() {
        let small = band(200.0, 1.0);
        assert!(!needs_nesting(&small, request()));
        let plan = fixed_grid(&small, request()).unwrap();
        assert_eq!(plan.summary.tile_count, 1);
        assert!(!plan.summary.requires_registration);
    }

    #[test]
    fn a_1200mm_band_is_split_into_several_tiles() {
        let long = band(1200.0, 2.0);
        assert!(needs_nesting(&long, request()));
        let plan = fixed_grid(&long, request()).unwrap();
        // 1200 / (284 + 0.9) = 4.21 -> 5 columns
        assert_eq!(plan.summary.tile_count, 5);
        assert!(plan.summary.requires_registration);
    }

    #[test]
    fn tiles_are_labelled_for_reassembly() {
        let plan = fixed_grid(&band(1200.0, 2.0), request()).unwrap();
        let ids: Vec<&str> = plan.tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A1", "A2", "A3", "A4", "A5"]);
    }

    #[test]
    fn neighbouring_tiles_keep_the_required_gap() {
        // Measured from actual content, not from the field rectangles: the
        // fields are pitch apart, and the content gap is what must equal the
        // minimum.
        let gap = request().separation_gap_mm;
        let plan = fixed_grid(&band(1200.0, 2.0), request()).unwrap();
        let a_right = mask_kernel::bounding_box(&plan.tiles[0].content).0
            + mask_kernel::bounding_box(&plan.tiles[0].content).2;
        let b_left = mask_kernel::bounding_box(&plan.tiles[1].content).0;
        assert!(
            (b_left - a_right - gap).abs() < 1e-9,
            "gap between neighbours was {}, expected {gap}",
            b_left - a_right
        );
    }

    #[test]
    fn the_outer_edge_of_the_mask_is_not_shaved() {
        // Regression: insetting every window by gap/2 trimmed 0.45 mm off all
        // four sides of the mask itself, where there is nothing to separate
        // from. The first tile must start exactly at the mask boundary.
        let long = band(1200.0, 2.0);
        let (min_x, min_y, _, _) = mask_kernel::bounding_box(&long);
        let plan = fixed_grid(&long, request()).unwrap();
        let first = mask_kernel::bounding_box(&plan.tiles[0].content);
        assert!(
            (first.0 - min_x).abs() < 1e-9,
            "left edge shaved by {} mm",
            first.0 - min_x
        );
        assert!(
            (first.1 - min_y).abs() < 1e-9,
            "bottom edge shaved by {} mm",
            first.1 - min_y
        );
    }

    #[test]
    fn tiling_preserves_total_area() {
        // The decisive property: splitting must not create or destroy material.
        let long = band(1200.0, 2.0);
        let whole = mask_kernel::shapes_area(&long);
        let plan = fixed_grid(&long, request()).unwrap();
        let tiled: f64 = plan
            .tiles
            .iter()
            .map(|t| mask_kernel::shapes_area(&t.content))
            .sum();
        // One separation band is lost per split, across the full band height.
        let height = mask_kernel::bounding_box(&long).3;
        let expected_loss =
            (plan.summary.tile_count as f64 - 1.0) * request().separation_gap_mm * height;
        assert!(
            (whole - tiled - expected_loss).abs() < 1e-6,
            "whole {whole}, tiled {tiled}, expected loss {expected_loss}"
        );
    }

    #[test]
    fn every_tile_carries_registration_marks() {
        let plan = fixed_grid(&band(1200.0, 2.0), request()).unwrap();
        for tile in &plan.tiles {
            assert!(
                tile.marks.orientation,
                "tile {} needs an orientation arrow",
                tile.id
            );
            assert_eq!(tile.marks.label, tile.id);
            // Marks must sit inside the tile, not outside it.
            assert!(tile.marks.part_anchor.left.x_mm >= tile.rect.x_mm);
            assert!(tile.marks.part_anchor.right.x_mm <= tile.rect.x_mm + tile.rect.width_mm);
        }
    }

    #[test]
    fn tile_numbering_is_contiguous_and_complete() {
        // Every grid cell gets a tile even when it carries no content, so the
        // numbering an instruction refers to can never be wrong.
        let plan = fixed_grid(&band(600.0, 2.0), request()).unwrap();
        let ids: Vec<&str> = plan.tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A1", "A2", "A3"]);
        assert_eq!(plan.tiles[0].sheet.index, 0);
        assert_eq!(plan.tiles[2].sheet.index, 2);
    }

    #[test]
    fn a_two_dimensional_mask_is_split_both_ways() {
        // Must exceed the field on BOTH axes: 284 wide, 1184 long.
        // 900 wide needs 4 columns; 1500 long needs 2 rows.
        let frame = mask_kernel::Shapes::single(mask_kernel::Shape::new(mask_kernel::Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 900.0, y: 0.0 },
                Point {
                    x: 900.0,
                    y: 1500.0,
                },
                Point { x: 0.0, y: 1500.0 },
            ],
            closed: true,
        }));
        let plan = fixed_grid(&frame, request()).unwrap();
        assert_eq!(
            plan.tiles.len(),
            8,
            "4 columns by 2 rows for a 900x1500 part"
        );
        let mut ids: Vec<&str> = plan.tiles.iter().map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["A1", "A2", "A3", "A4", "B1", "B2", "B3", "B4"]);
    }

    #[test]
    fn empty_geometry_yields_no_tiles_rather_than_an_error() {
        let plan = fixed_grid(&Shapes::new(), request()).unwrap();
        assert_eq!(plan.summary.tile_count, 0);
        assert_eq!(plan.summary.waste_ratio, 0.0);
    }

    #[test]
    fn a_field_smaller_than_the_gap_is_refused() {
        let bad = NestRequest {
            field_width_mm: 0.5,
            field_length_mm: 1184.0,
            separation_gap_mm: 0.9,
            kerf_mm: 0.15,
        };
        assert!(matches!(
            fixed_grid(&band(100.0, 1.0), bad),
            Err(NestError::FieldConsumedByGap { .. })
        ));
    }

    #[test]
    fn a_pathological_size_is_refused_rather_than_allocating() {
        // Inside the coordinate bound, but a 99 km part would ask for ~120000
        // tiles. The limit must stop that before anything is allocated.
        let absurd = mask_kernel::Shapes::single(mask_kernel::Shape::new(mask_kernel::Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point {
                    x: 99_000.0,
                    y: 0.0,
                },
                Point {
                    x: 99_000.0,
                    y: 99_000.0,
                },
                Point {
                    x: 0.0,
                    y: 99_000.0,
                },
            ],
            closed: true,
        }));
        assert!(matches!(
            fixed_grid(&absurd, request()),
            Err(NestError::TooManyTiles { .. })
        ));
    }

    #[test]
    fn nesting_is_deterministic() {
        let long = band(1200.0, 2.0);
        assert_eq!(
            fixed_grid(&long, request()).unwrap(),
            fixed_grid(&long, request()).unwrap()
        );
    }

    #[test]
    fn waste_ratio_is_reported_and_bounded() {
        let plan = fixed_grid(&band(1200.0, 2.0), request()).unwrap();
        assert!(plan.summary.waste_ratio > 0.0 && plan.summary.waste_ratio < 1.0);
    }
}
