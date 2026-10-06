//! Checks the three exporters against a curved disc: how many points each file
//! carries, whether each says it flattened, and what that looks like.
use mask_export::{dxf, plt};
use mask_kernel::geom::{ArcSegment, Contour, Point, Shape, Shapes};

fn disc(r: f64) -> Shapes {
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

fn main() {
    let shapes = disc(20.0);
    let tol = 0.08;

    let dxf = dxf::to_dxf(
        &dxf::DxfContent {
            cut: shapes.clone(),
            registration: Shapes::new(),
        },
        tol,
    );
    let device = plt::PltDevice {
        size_code: 10,
        units_per_mm: 40.0,
        speed_straight: 30,
        speed_corner: 18,
        blade_offset_mm: 0.10,
        kerf_mm: 0.15,
        curve_tolerance_mm: tol,
    };
    let plt_text = plt::to_plt(
        &plt::PltContent {
            cut: shapes.clone(),
            registration: Shapes::new(),
        },
        device,
        "arcs",
    );

    println!(
        "точек в исходном контуре: {}",
        shapes.shapes[0].outer.points.len()
    );
    println!(
        "вершин в DXF:           {}",
        dxf.matches("\nVERTEX\r\n").count()
    );
    println!("перемещений в PLT:      {}", plt_text.matches("PA").count());
    println!(
        "DXF: {}",
        dxf.lines()
            .find(|l| l.contains("flattened"))
            .unwrap_or("НЕ ОБЪЯВЛЯЕТ")
    );
    println!(
        "PLT: {}",
        plt_text
            .lines()
            .find(|l| l.contains("curve-tolerance"))
            .unwrap_or("НЕ ОБЪЯВЛЯЕТ")
    );

    std::fs::write(
        "/tmp/vis/export_curves.svg",
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-24 -24 48 48" width="700" height="700">
<rect x="-24" y="-24" width="48" height="48" fill="white"/>
<path d="{}" fill="none" stroke="#1f6feb" stroke-width="0.7"/>
<path d="M 20 0 L 0 20 L -20 0 L 0 -20 Z" fill="none" stroke="#d1242f" stroke-width="0.7"/>
</svg>"##,
            mask_kernel::to_path_data(&shapes.flattened(tol).shapes[0].outer)
        ),
    )
    .expect("write svg");
    println!("/tmp/vis/export_curves.svg — синий: кривая при допуске 0.08 мм, красный: хорды");
}
