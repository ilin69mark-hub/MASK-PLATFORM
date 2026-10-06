//! Rounds a square's corners and draws the result two ways: as the arcs it now
//! holds, and as the chords an earlier version produced.
use mask_kernel::geom::{Contour, Point, Shape, Shapes};
fn main() {
    let sq = Shapes::single(Shape::new(Contour {
        points: vec![
            Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 10.0 }, Point { x: 0.0, y: 10.0 },
        ],
        closed: true, arcs: Vec::new(),
    }));
    let (rounded, report) = mask_kernel::round_corners(&sq, 1.0, 100.0, 0.05);
    let c = &rounded.shapes[0].outer;
    println!("{report:?}");
    println!("точек: {}, дуг: {}", c.points.len(), c.arcs.len());
    for a in &c.arcs { println!("  дуга {}..{} центр=({:.4},{:.4}) r={:.4}", a.from, a.to, a.centre.x, a.centre.y, a.radius); }
    println!("площадь (точная, по дугам): {:.6}", mask_kernel::measure::shapes_area(&rounded));
    println!("площадь 100-(4-pi):          {:.6}", 100.0 - (4.0 - std::f64::consts::PI));

    std::fs::write("/tmp/vis/corners.svg", format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-1 -1 12 12" width="760" height="760">
<rect x="-1" y="-1" width="12" height="12" fill="white"/>
<path d="{}" fill="none" stroke="#1f6feb" stroke-width="0.05"/>
<path d="{}" fill="none" stroke="#d1242f" stroke-width="0.05"/>
<path d="M 0 0 L 10 0 L 10 10 L 0 10 Z" fill="none" stroke="#999" stroke-width="0.02"/>
</svg>"##,
        mask_kernel::to_path_data(c),
        mask_kernel::to_path_data(&rounded.flattened(0.08).shapes[0].outer))).expect("write");
    println!("/tmp/vis/corners.svg — синий: дуги (то, что теперь в геометрии), красный: развёртка при 0.08 мм, серый: исходный квадрат");
}
