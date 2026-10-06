//! Prints the hashes the golden file must contain, straight from the Rust side.
use mask_kernel::geom::{Contour, Point, Shape, Shapes};
use mask_kernel::ContentHash;
fn main() {
    let sq = |w: f64, h: f64| Shapes {
        shapes: vec![Shape::new(Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: w, y: 0.0 },
                Point { x: w, y: h },
                Point { x: 0.0, y: h },
            ],
            closed: true,
            arcs: Vec::new(),
        })],
    };
    println!("square_10x5 {}", ContentHash::of(&sq(10.0, 5.0)).digest);
    println!("square_7x3  {}", ContentHash::of(&sq(7.0, 3.0)).digest);
    for half in [0.21000003814697266_f64, 0.3999999761581421] {
        let band = Shapes {
            shapes: vec![Shape::new(Contour {
                points: vec![
                    Point { x: 0.0, y: half },
                    Point { x: 0.0, y: -half },
                    Point { x: 100.0, y: -half },
                    Point { x: 100.0, y: half },
                ],
                closed: true,
                arcs: Vec::new(),
            })],
        };
        println!("band_{half}  {}", ContentHash::of(&band).digest);
    }
    println!("scheme  {}", mask_kernel::HASH_SCHEME_VERSION);
}
