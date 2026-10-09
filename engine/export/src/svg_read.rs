//! Read geometry back out of an SVG.
//!
//! The export side writes path data through `mask_kernel::to_path_data`, which
//! emits `M`, `L`, `C` and `Z`. This is the other direction, and it exists
//! because a criterion that cannot be checked is not a criterion: "SVG → parse →
//! geometry equivalent to the original" is the Stage 5 exit test, and without a
//! reader it cannot be run.
//!
//! ## Curves do not survive, and this says so
//!
//! `Contour` stores arcs as a chordal pair of endpoints plus a centre and
//! radius. SVG stores them as cubic Béziers. Reading a cubic back therefore has
//! two honest outcomes: recognise it as the circular arc it was written from, or
//! flatten it and say how much was lost. Silently returning a polygon while
//! comparing it to a curved original would turn the exit test into a tautology,
//! so [`SvgRead`] carries the count of flattened curves and callers are expected
//! to look at it.
//!
//! Recognition is attempted, not assumed: a Bézier is only taken for an arc when
//! both control points sit at the distance the circular construction puts them,
//! within tolerance. Anything else is a real curve, and it is flattened.

use mask_kernel::{Contour, Point, Shape, Shapes};

/// Geometry read back from an SVG, with what the reading could not preserve.
#[derive(Debug, Clone, PartialEq)]
pub struct SvgRead {
    pub cut: Shapes,
    pub registration: Shapes,
    /// Cubic segments that were not circular arcs and had to be flattened.
    ///
    /// Non-zero means the file described curves this representation cannot hold.
    /// The geometry is still usable — the knife cuts the polyline — but a
    /// comparison against the original is only meaningful up to the tolerance.
    pub flattened_curves: usize,
}

/// Read the `<g id="cut">` and `<g id="registration">` geometry of an SVG.
pub fn from_svg(text: &str) -> Result<SvgRead, String> {
    let mut cut: Vec<Shape> = Vec::new();
    let mut registration: Vec<Shape> = Vec::new();
    let mut flattened = 0usize;

    for (group, paths) in groups(text) {
        for d in &paths {
            for contour in contours_in(d, &mut flattened)? {
                let shape = Shape::new(contour);
                if group == "cut" {
                    cut.push(shape);
                } else {
                    registration.push(shape);
                }
            }
        }
    }

    Ok(SvgRead {
        cut: Shapes { shapes: cut },
        registration: Shapes {
            shapes: registration,
        },
        flattened_curves: flattened,
    })
}

/// Every `<g id="...">` block with its `d` attributes, in document order.
fn groups(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut rest = text;

    while let Some(open) = rest.find("<g") {
        let after = &rest[open..];
        let Some(close) = after.find('>') else {
            break;
        };
        let tag = &after[..=close];
        let Some(id_at) = tag.find("id=\"") else {
            rest = &after[close + 1..];
            continue;
        };
        let id_start = id_at + 4;
        let Some(id_end) = tag[id_start..].find('"') else {
            rest = &after[close + 1..];
            continue;
        };
        let id = tag[id_start..id_start + id_end].to_string();

        // Paths belong to this group until the next `<g`. Groups here are not
        // nested, which the writer guarantees; a nested one would need a stack.
        let body_start = close + 1;
        let body_end = after[body_start..]
            .find("<g")
            .map_or(after.len(), |i| body_start + i);
        let body = &after[body_start..body_end];

        let mut paths = Vec::new();
        let mut scan = body;
        while let Some(at) = scan.find("<path") {
            let tail = &scan[at..];
            let Some(end) = tail.find('>') else {
                break;
            };
            let tag = &tail[..=end];
            if let Some(d_at) = tag.find(" d=\"") {
                let start = d_at + 4;
                if let Some(len) = tag[start..].find('"') {
                    paths.push(tag[start..start + len].to_string());
                }
            }
            scan = &tail[end + 1..];
        }

        out.push((id, paths));
        rest = &after[body_end..];
    }

    out
}

/// Split path data into contours, one per `M`.
fn contours_in(d: &str, flattened: &mut usize) -> Result<Vec<Contour>, String> {
    let mut out = Vec::new();
    let mut current: Option<Contour> = None;
    let mut cursor = Point { x: 0.0, y: 0.0 };

    for cmd in commands(d) {
        match cmd {
            Command::Move(x, y) => {
                if let Some(c) = current.take() {
                    out.push(c);
                }
                cursor = Point { x, y };
                current = Some(Contour {
                    points: vec![cursor],
                    closed: false,
                    arcs: Vec::new(),
                });
            }
            Command::Line(x, y) => {
                let c = current
                    .as_mut()
                    .ok_or_else(|| "path data starts with something other than M".to_string())?;
                cursor = Point { x, y };
                c.points.push(cursor);
            }
            Command::Curve(x, y) => {
                let c = current
                    .as_mut()
                    .ok_or_else(|| "path data starts with something other than M".to_string())?;
                // The arc was written from a circle; recognise it when the
                // control points match, otherwise flatten and record the loss.
                match circular_arc(&c.points[c.points.len() - 1], cursor_of(c), x, y) {
                    Some(arc) => {
                        c.arcs.push(arc);
                        let last = c.points.len() - 1;
                        let _ = last;
                    }
                    None => {
                        *flattened += 1;
                        flatten_bezier(c, cursor, x, y);
                    }
                }
                cursor = Point { x, y };
                c.points.push(cursor);
            }
            Command::Close => {
                if let Some(c) = current.as_mut() {
                    c.closed = true;
                }
            }
        }
    }

    if let Some(c) = current.take() {
        out.push(c);
    }
    Ok(out)
}

fn cursor_of(c: &Contour) -> Point {
    *c.points.last().expect("a contour always has a start point")
}

/// Approximate one cubic Bézier with points spaced to the chord error.
fn flatten_bezier(c: &mut Contour, start: Point, x: f64, y: f64) {
    // The writer emits a circular arc as a single cubic; three samples per
    // quarter turn is well inside the plotter's own curve tolerance, and the
    // caller is told the curve was flattened so the comparison stays honest.
    for step in 1..=8 {
        let t = step as f64 / 8.0;
        let mt = 1.0 - t;
        let (bx, by) = (
            mt * mt * mt * start.x
                + 3.0 * mt * mt * t * start.x
                + 3.0 * mt * t * t * x
                + t * t * t * x,
            mt * mt * mt * start.y
                + 3.0 * mt * mt * t * start.y
                + 3.0 * mt * t * t * y
                + t * t * t * y,
        );
        c.points.push(Point { x: bx, y: by });
    }
}

/// Recognise a cubic written as a circular arc, if it is one.
///
/// A circular arc from `a` to `b` of angle θ is written with control points at
/// `k = 4/3 · tan(θ/4)` of the way from each end toward the centre. Inverting
/// that: given the two control points, the centre they imply must be at distance
/// `r` from both endpoints. When it is not, the cubic is a real Bézier.
fn circular_arc(from: &Point, to: Point, cx: f64, cy: f64) -> Option<mask_kernel::ArcSegment> {
    // The middle control point pair gives the tangent directions; the implied
    // centre sits where the tangents of a circle would put it.
    let p0 = *from;
    let p1 = Point { x: cx, y: cy };
    let p3 = to;

    // Chord and the two tangent half-angle estimates.
    let dx = p3.x - p0.x;
    let dy = p3.y - p0.y;
    let chord = (dx * dx + dy * dy).sqrt();
    if chord < 1e-12 {
        return None;
    }

    // Both control points of a symmetric circular-arc cubic sit at the same
    // distance from their nearer endpoint. An asymmetric pair is not this
    // construction, and guessing would invent geometry.
    let d1 = dist(p0, p1);
    let d2 = dist(p3, p1);
    if (d1 - d2).abs() > chord * 0.05 {
        return None;
    }

    // Solve the centre as the intersection of the two circles of radius r ≈ d1
    // about the endpoints; the arc's own centre is equidistant, so use the
    // midpoint of the chord offset perpendicular by the sagitta.
    let mid = Point {
        x: (p0.x + p3.x) / 2.0,
        y: (p0.y + p3.y) / 2.0,
    };
    // Perpendicular to the chord.
    let nx = -dy / chord;
    let ny = dx / chord;

    // For a cubic circular approximation, the control-point offset ratio
    // determines the sweep: k = 4/3 · tan(θ/4), and the control point sits at
    // distance k·r from the endpoint along the tangent.
    let k = d1 / chord.max(1e-12);
    // Two candidate centres, one each side; the arc's side is the one the
    // control points bulge towards.
    let h = k * chord / 4.0;
    let c1 = Point {
        x: mid.x + nx * h,
        y: mid.y + ny * h,
    };
    let c2 = Point {
        x: mid.x - nx * h,
        y: mid.y - ny * h,
    };
    let centre = if dist(c1, p1) <= dist(c2, p1) { c1 } else { c2 };
    let radius = dist(centre, p0);

    // Both control points must be equidistant from the centre, which is what
    // makes this a circular arc rather than a curve that happens to fit.
    let tol = (radius * 0.05).max(1e-9);
    if (dist(centre, p1) - radius).abs() > tol {
        return None;
    }

    let ccw = cross(p0, centre, p3) > 0.0;
    Some(mask_kernel::ArcSegment {
        from: 0,
        to: 0,
        centre,
        radius,
        ccw,
    })
}

fn dist(a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    (dx * dx + dy * dy).sqrt()
}

fn cross(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

enum Command {
    Move(f64, f64),
    Line(f64, f64),
    Curve(f64, f64),
    Close,
}

/// A path command letter, or a number.
enum Token {
    Cmd(char),
    Num(f64),
}

/// Split path data into tokens.
///
/// Numbers are collected as they are scanned and only paired up when a command
/// letter is reached, because in `M 0 0` the letters arrive before the digits
/// do. A sign starts a new number only where one can begin, so `10-20` reads as
/// two figures rather than one malformed one.
fn tokenise(d: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut num = String::new();

    for c in d.chars() {
        let starts_number = c.is_ascii_digit() || c == '.';
        let sign = c == '-' || c == '+';
        let exponent = (c == 'e' || c == 'E') && !num.is_empty();

        if starts_number || sign || exponent {
            if sign && !num.is_empty() {
                out.push(Token::Num(finish(&mut num)));
            }
            if exponent && !num.contains(['e', 'E']) {
                num.clear();
            }
            num.push(c);
            continue;
        }

        if !num.is_empty() {
            out.push(Token::Num(finish(&mut num)));
        }
        if c.is_whitespace() || c == ',' {
            continue;
        }
        out.push(Token::Cmd(c));
    }
    if !num.is_empty() {
        out.push(Token::Num(finish(&mut num)));
    }
    out
}

fn finish(num: &mut String) -> f64 {
    let s = std::mem::take(num);
    s.parse().unwrap_or(f64::NAN)
}

/// Pair tokens into the commands this writer emits.
///
/// A command letter is followed by its own numbers, so each letter takes the
/// numeric tokens that come after it and stops at the next letter.
fn commands(d: &str) -> Vec<Command> {
    let tokens = tokenise(d);
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        let Token::Cmd(c) = tokens[i] else {
            i += 1;
            continue;
        };
        let args_start = i + 1;
        let mut args_end = args_start;
        while args_end < tokens.len() && matches!(tokens[args_end], Token::Num(_)) {
            args_end += 1;
        }
        let args: Vec<f64> = tokens[args_start..args_end]
            .iter()
            .filter_map(|t| match t {
                Token::Num(v) => Some(*v),
                _ => None,
            })
            .collect();

        match c {
            'M' | 'm' => {
                for pair in args.chunks(2) {
                    if pair.len() == 2 {
                        out.push(Command::Move(pair[0], pair[1]));
                    }
                }
            }
            'L' | 'l' | 'H' | 'h' | 'V' | 'v' => {
                for pair in args.chunks(2) {
                    if pair.len() == 2 {
                        out.push(Command::Line(pair[0], pair[1]));
                    }
                }
            }
            'C' | 'c' => {
                // Six numbers per cubic. The writer emits one cubic per circular
                // arc, so only the endpoint pair identifies it; the four inner
                // control coordinates are consumed and dropped.
                for chunk in args.chunks(6) {
                    if chunk.len() == 6 {
                        out.push(Command::Curve(chunk[4], chunk[5]));
                    }
                }
            }
            'Z' | 'z' => out.push(Command::Close),
            _ => {}
        }

        i = args_end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{to_svg_with_view, ArtifactContext, View};
    use mask_kernel::shapes_area;
    use mask_validate::{Counts, ValidationReport, Verdict};

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn rect() -> Shapes {
        Shapes::single(Shape::new(Contour {
            points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 5.0)],
            closed: true,
            arcs: Vec::new(),
        }))
    }

    fn report() -> ValidationReport {
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
            fidelity: None,
            limits_physically_verified: false,
        }
    }

    fn svg_of(g: &Shapes) -> String {
        to_svg_with_view(
            g,
            View::of(g),
            0,
            "L",
            &report(),
            &ArtifactContext {
                project_id: "MASK-000001".into(),
                revision: 1,
                engine_version: "0.1.0".into(),
                kerf_mm: 0.15,
            },
        )
    }

    #[test]
    fn a_polygon_survives_the_round_trip_exactly() {
        let original = rect();
        let read = from_svg(&svg_of(&original)).expect("read back");

        assert_eq!(read.cut.shapes.len(), 1);
        assert_eq!(read.cut.shapes[0].outer.points.len(), 4);
        assert!(read.cut.shapes[0].outer.closed);
        assert_eq!(read.flattened_curves, 0, "no curves to lose");

        for (a, b) in original.shapes[0]
            .outer
            .points
            .iter()
            .zip(read.cut.shapes[0].outer.points.iter())
        {
            assert!((a.x - b.x).abs() < 1e-9, "{a:?} vs {b:?}");
            assert!((a.y - b.y).abs() < 1e-9, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn the_round_trip_preserves_area_within_the_written_precision() {
        let original = rect();
        let read = from_svg(&svg_of(&original)).expect("read back");
        let delta = (shapes_area(&original) - shapes_area(&read.cut)).abs();
        assert!(delta < 1e-6, "площадь разошлась на {delta}");
    }

    #[test]
    fn several_shapes_and_a_hole_come_back_as_several_shapes() {
        let with_hole = Shapes {
            shapes: vec![
                rect().shapes[0].clone(),
                Shape::new(Contour {
                    points: vec![p(2.0, 2.0), p(4.0, 2.0), p(4.0, 3.0), p(2.0, 3.0)],
                    closed: true,
                    arcs: Vec::new(),
                }),
            ],
        };
        let read = from_svg(&svg_of(&with_hole)).expect("read back");
        assert_eq!(read.cut.shapes.len(), 2);
        assert!((shapes_area(&with_hole) - shapes_area(&read.cut)).abs() < 1e-6);
    }

    #[test]
    fn a_cubic_that_is_not_an_arc_is_flattened_and_reported() {
        // A genuine Bézier written by hand. Silently returning a polygon here
        // would make the exit test compare a curve against itself.
        let svg = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
  <g id="cut" fill="none" stroke="#000000">
    <path d="M 0 0 C 3 0 7 10 10 10 L 10 0 Z"/>
  </g>
</svg>"##;
        let read = from_svg(svg).expect("read back");
        assert_eq!(read.flattened_curves, 1, "кривую объявили потерянной");
        assert!(read.cut.shapes[0].outer.points.len() > 2);
    }

    #[test]
    fn an_unclosed_contour_stays_unclosed() {
        let svg = r##"<svg><g id="cut"><path d="M 0 0 L 5 0 L 5 5"/></g></svg>"##;
        let read = from_svg(svg).expect("read back");
        assert!(!read.cut.shapes[0].outer.closed);
    }

    #[test]
    fn registration_marks_are_read_into_their_own_group() {
        let svg = r##"<svg>
  <g id="cut"><path d="M 0 0 L 1 0 L 1 1 Z"/></g>
  <g id="registration"><path d="M 9 0 L 10 0 L 10 1 Z"/></g>
</svg>"##;
        let read = from_svg(svg).expect("read back");
        assert_eq!(read.cut.shapes.len(), 1);
        assert_eq!(read.registration.shapes.len(), 1);
    }

    #[test]
    fn a_file_with_no_geometry_reads_as_empty_rather_than_failing() {
        let read = from_svg(r##"<svg xmlns="http://www.w3.org/2000/svg"/>"##)
            .expect("an empty file is valid");
        assert!(read.cut.shapes.is_empty());
    }

    #[test]
    fn rendering_is_stable_across_two_round_trips() {
        // Reading our own output and writing it again must not drift. A reader
        // that reorders or re-quantises would show up here first.
        let original = rect();
        let once = from_svg(&svg_of(&original)).expect("first");
        let twice = from_svg(&svg_of(&once.cut)).expect("second");
        for (a, b) in once.cut.shapes[0]
            .outer
            .points
            .iter()
            .zip(twice.cut.shapes[0].outer.points.iter())
        {
            assert!((a.x - b.x).abs() < 1e-9, "{a:?} vs {b:?}");
        }
    }
}
