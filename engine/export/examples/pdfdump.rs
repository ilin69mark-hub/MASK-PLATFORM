//! Write a one-page PDF whose only content is cut geometry, so a renderer shows
//! the layer rather than the title page.

use mask_kernel::{Contour, Point};

fn main() {
    let rect = Contour {
        points: vec![
            Point { x: 10.0, y: 10.0 },
            Point { x: 60.0, y: 10.0 },
            Point { x: 60.0, y: 40.0 },
            Point { x: 10.0, y: 40.0 },
        ],
        closed: true,
        arcs: Vec::new(),
    };
    let meta = mask_export::pdf::SheetMeta {
        project_id: "MASK-000001",
        revision: 2,
        engine_version: "0.1.0",
        ..mask_export::pdf::SheetMeta::default()
    };
    let layer = mask_export::pdf::SheetLayer {
        index: 1,
        name: "L",
        paths: std::slice::from_ref(&rect),
        fills: &[],
    };
    let content = mask_export::pdf::layer_page_content(&layer, &meta);
    let pdf = mask_export::pdf::pages_pdf(
        &[(
            "MASK-000001 - layer 1 - PASS - LIMITS UNPROVEN".to_string(),
            content,
        )],
        &meta,
    );
    std::fs::write("/tmp/probe.pdf", &pdf).unwrap();
    println!("{} байт", pdf.len());
}
