//! Integration checks for nesting that need the public API from outside the
//! crate, and that assert the property most likely to break silently: a split
//! must not create or destroy material.

use mask_kernel::{bounding_box, shapes_area, widen, Point};
use mask_nesting::{fixed_grid, needs_nesting, NestRequest};

fn p(x: f64, y: f64) -> Point {
    Point { x, y }
}

fn request() -> NestRequest {
    NestRequest {
        field_width_mm: 284.0,
        field_length_mm: 1184.0,
        separation_gap_mm: 0.9,
        kerf_mm: 0.15,
    }
}

#[test]
fn splitting_a_long_band_loses_exactly_one_gap_per_split() {
    let band = widen(&[p(0.0, 0.0), p(1200.0, 0.0)], 2.0).unwrap();
    let whole = shapes_area(&band);
    let (_, _, _, height) = bounding_box(&band);

    let plan = fixed_grid(&band, request()).unwrap();
    let tiled: f64 = plan.tiles.iter().map(|t| shapes_area(&t.content)).sum();

    let splits = plan.summary.tile_count as f64 - 1.0;
    let expected_loss = splits * request().separation_gap_mm * height;

    assert!(
        (whole - tiled - expected_loss).abs() < 1e-6,
        "whole {whole}, tiled {tiled}, expected loss {expected_loss}"
    );
}

#[test]
fn the_first_tile_starts_exactly_at_the_mask_edge() {
    // Regression: an earlier version inset every content window by half the
    // separation gap on all four sides, which shaved 0.45 mm off the outer
    // boundary of the mask itself and silently destroyed 546 mm2 of a
    // 2400 mm2 band. The outer edge is not a split and needs no clearance.
    let band = widen(&[p(0.0, 0.0), p(1200.0, 0.0)], 2.0).unwrap();
    let (min_x, min_y, _, _) = bounding_box(&band);
    let plan = fixed_grid(&band, request()).unwrap();
    let first = bounding_box(&plan.tiles[0].content);

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
fn every_tile_content_fits_inside_the_machine_field() {
    let frame = mask_kernel::Shapes::single(mask_kernel::Shape::new(mask_kernel::Contour {
        points: vec![p(0.0, 0.0), p(900.0, 0.0), p(900.0, 1500.0), p(0.0, 1500.0)],
        closed: true,
    }));
    let plan = fixed_grid(&frame, request()).unwrap();
    assert!(needs_nesting(&frame, request()));

    for tile in &plan.tiles {
        let (_, _, w, h) = bounding_box(&tile.content);
        assert!(
            w <= request().field_width_mm + 1e-9,
            "tile {} is {} mm wide, wider than the field",
            tile.id,
            w
        );
        assert!(
            h <= request().field_length_mm + 1e-9,
            "tile {} is {} mm long, longer than the field",
            tile.id,
            h
        );
    }
}

#[test]
fn nesting_is_reproducible() {
    let band = widen(&[p(0.0, 0.0), p(1200.0, 0.0)], 2.0).unwrap();
    assert_eq!(
        fixed_grid(&band, request()).unwrap(),
        fixed_grid(&band, request()).unwrap()
    );
}
