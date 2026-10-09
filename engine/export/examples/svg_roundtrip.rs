//! Read every produced SVG back and check it against an independent reader.
//!
//! `to_dxf` and `to_svg` are written from the same `Shapes`. The DXF reader is
//! older and already trusted by its own round-trip test. So reading both files
//! and comparing areas compares the new SVG reader against a reader nobody is
//! currently questioning — which is a stronger statement than "the SVG parses".
//!
//!     make physical
//!     cargo run --release -p mask-export --example svg_roundtrip -- out/physical

use mask_export::{dxf, svg_read};
use mask_kernel::shapes_area;

fn main() -> Result<(), String> {
    let dir = std::env::args().nth(1).ok_or("нужна папка с SVG и DXF")?;
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("cannot read {dir}: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "svg"))
        .collect();
    files.sort();

    if files.is_empty() {
        return Err(format!("в {dir} нет ни одного SVG"));
    }

    let mut worst = 0.0f64;
    let mut worst_file = String::new();
    let mut flattened = 0usize;
    let mut compared = 0usize;

    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let svg_text = std::fs::read_to_string(f).map_err(|e| format!("{name}: {e}"))?;
        let svg = svg_read::from_svg(&svg_text).map_err(|e| format!("{name}: {e}"))?;
        let svg_area = shapes_area(&svg.cut);
        flattened += svg.flattened_curves;

        let dxf_path = f.with_extension("dxf");
        if !dxf_path.exists() {
            println!("{name}: DXF нет, только прочитан SVG: {svg_area:.2} мм²");
            continue;
        }
        let dxf_text = std::fs::read_to_string(&dxf_path).map_err(|e| format!("{name}: {e}"))?;
        let dxf = dxf::from_dxf(&dxf_text).map_err(|e| format!("{name} (dxf): {e}"))?;
        let dxf_area = shapes_area(&dxf.cut);
        compared += 1;

        let delta = (svg_area - dxf_area).abs();
        if delta > worst {
            worst = delta;
            worst_file = name.clone();
        }
        println!(
            "{name}: {} shape(s) · SVG {svg_area:.4} мм² · DXF {dxf_area:.4} мм² · Δ {delta:.2e}",
            svg.cut.shapes.len()
        );
    }

    println!(
        "\n{compared} файл(ов) сверено с DXF, {flattened} кривых сплющено, худшее расхождение {worst:.2e} мм² ({worst_file})"
    );
    if worst > 1e-6 {
        return Err(format!(
            "SVG и DXF расходятся на {worst:.6} мм² — это не округление"
        ));
    }
    Ok(())
}
