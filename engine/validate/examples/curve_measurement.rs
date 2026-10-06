//! The same disc twice: as arcs, and as the polygon inside them. `min_width`
//! bisects on inward offsets, so it reads whichever boundary it is handed.
use mask_kernel::geom::{ArcSegment, Contour, Point, Shape, Shapes};

const TOLERANCE_MM: f64 = 0.08; // the blade profile's curve tolerance

fn disc(r: f64, with_arcs: bool) -> Shapes {
    const N: usize = 16;
    let points = (0..N)
        .map(|i| {
            let a = std::f64::consts::TAU * (i as f64 / N as f64);
            Point {
                x: r * a.cos(),
                y: r * a.sin(),
            }
        })
        .collect::<Vec<_>>();
    let arcs = if with_arcs {
        (0..N)
            .map(|i| ArcSegment {
                from: i,
                to: (i + 1) % N,
                centre: Point { x: 0.0, y: 0.0 },
                radius: r,
                ccw: true,
            })
            .collect()
    } else {
        Vec::new()
    };
    Shapes::single(Shape::new(Contour {
        points,
        closed: true,
        arcs,
    }))
}

fn main() {
    let as_chords = mask_kernel::to_path_data(&disc(3.0, false).shapes[0].outer);
    let flattened =
        mask_kernel::to_path_data(&disc(3.0, true).flattened(TOLERANCE_MM).shapes[0].outer);

    println!("истина                    6.0000 мм");
    println!(
        "развёрнуто {TOLERANCE_MM} мм -> {:.4} мм",
        mask_kernel::min_width(&disc(3.0, true).flattened(TOLERANCE_MM)).expect("width")
    );
    println!(
        "хорды (16-угольник)      -> {:.4} мм",
        mask_kernel::min_width(&disc(3.0, false)).expect("width")
    );

    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-3.6 -3.6 7.2 7.2" width="760" height="760">
<rect x="-3.6" y="-3.6" width="7.2" height="7.2" fill="white"/>
<path d="M -3 0 H 3" stroke="#ccc" stroke-width="0.03"/>
<path d="M 0 -3 V 3" stroke="#ccc" stroke-width="0.03"/>
<path d="{flattened}" fill="none" stroke="#1f6feb" stroke-width="0.05"/>
<path d="{as_chords}" fill="none" stroke="#d1242f" stroke-width="0.05"/>
</svg>"##
    );
    std::fs::write("/tmp/vis/measure_width.svg", svg).expect("write svg");
    println!("(/tmp/vis/measure_width.svg: синий — развёрнуто, красный — хорды)");
}
