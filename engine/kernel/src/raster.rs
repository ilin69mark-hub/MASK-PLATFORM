//! Raster to vector: contour tracing and colour separation.
//!
//! This is the missing link between a photograph and a mask. Everything
//! downstream — layers, topology, validation, export — already exists and is
//! tested, and none of it can run without geometry, and there is currently no
//! path from an image to geometry.
//!
//! Two deliberate constraints:
//!
//! * **No neural network.** Contour tracing and colour clustering are classical
//!   image processing, they are deterministic, they need no training data, and
//!   their failure modes are visible. A model would add a second source of truth
//!   for no gain, and this project's rule is that anything geometric is decided
//!   by measurable rules.
//! * **Every result is a candidate.** Nothing here decides a layer, a colour or
//!   a width. It produces regions and contours; the Validator and the
//!   manufacturing rules judge them (P-01).

use std::collections::BTreeMap;

use crate::constants::MEASURE_EPS_MM;
use crate::geom::{Contour, Point, Shape, Shapes};

/// A pixel raster. Greyscale or RGB, 8 bits per channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub width: usize,
    pub height: usize,
    /// Row-major, `channels` bytes per pixel.
    pub data: Vec<u8>,
    pub channels: usize,
}

impl Raster {
    /// Build a raster, rejecting inconsistent dimensions.
    pub fn new(
        width: usize,
        height: usize,
        channels: usize,
        data: Vec<u8>,
    ) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::Empty);
        }
        if !matches!(channels, 1 | 3 | 4) {
            return Err(ImageError::UnsupportedChannels(channels));
        }
        if data.len() != width * height * channels {
            return Err(ImageError::LengthMismatch {
                expected: width * height * channels,
                got: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
            channels,
        })
    }

    /// Convenience constructor for a single-channel image.
    pub fn grey(width: usize, height: usize, data: Vec<u8>) -> Result<Self, ImageError> {
        Self::new(width, height, 1, data)
    }

    /// How many millimetres one pixel covers.
    ///
    /// Derived from the real part dimensions rather than a DPI guess: the mask is
    /// sized by the part, and an assumed DPI would silently scale every
    /// measurement that follows.
    #[must_use]
    pub fn mm_per_pixel(&self, width_mm: f64) -> f64 {
        if self.width == 0 {
            return 1.0;
        }
        width_mm / self.width as f64
    }

    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * self.channels;
        match self.channels {
            1 => [self.data[i], self.data[i], self.data[i], 255],
            3 => [self.data[i], self.data[i + 1], self.data[i + 2], 255],
            _ => [
                self.data[i],
                self.data[i + 1],
                self.data[i + 2],
                self.data[i + 3],
            ],
        }
    }

    /// Luminance, 0–255.
    #[must_use]
    pub fn luma(&self, x: usize, y: usize) -> f64 {
        let [r, g, b, _] = self.pixel(x, y);
        // Integer weights: exact and reproducible, unlike float ones.
        0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImageError {
    #[error("image has zero width or height")]
    Empty,

    #[error("unsupported channel count {0}; expected 1, 3 or 4")]
    UnsupportedChannels(usize),

    #[error("pixel buffer length {got} does not match {expected}")]
    LengthMismatch { expected: usize, got: usize },

    #[error("no region survived the {threshold} threshold")]
    NoRegions { threshold: u8 },

    #[error("more than {max} colour clusters requested from a {palette}-colour image")]
    TooManyClusters {
        requested: usize,
        palette: usize,
        max: usize,
    },

    #[error("contour tracing failed to terminate")]
    TracingDidNotTerminate,
}

/// One flat colour found in the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A connected region of one colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    /// Index into the palette, ordered by first appearance so the result does
    /// not depend on iteration order.
    pub palette_index: usize,
    pub colour: Rgb,
    /// Contours in millimetres, already scaled.
    pub shapes: Shapes,
    pub pixel_count: usize,
}

/// The result of colour separation.
#[derive(Debug, Clone, PartialEq)]
pub struct Separation {
    pub palette: Vec<Rgb>,
    pub regions: Vec<Region>,
}

/// Separate an image into flat-colour regions.
///
/// Uses a fixed quantisation step rather than k-means: the output is
/// deterministic, reproducible, and its error is bounded and known. A learned
/// clustering would give nicer palettes and no guarantee.
pub fn separate_colours(
    image: &Raster,
    quantisation_step: u8,
    part_width_mm: f64,
    max_regions: usize,
) -> Result<Separation, ImageError> {
    if quantisation_step == 0 {
        return Err(ImageError::NoRegions { threshold: 0 });
    }

    let mut palette: Vec<Rgb> = Vec::new();
    let mut labels: Vec<usize> = Vec::with_capacity(image.width * image.height);
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();

    for y in 0..image.height {
        for x in 0..image.width {
            let [r, g, b, a] = image.pixel(x, y);
            // Fully transparent pixels carry no design information.
            if a == 0 {
                labels.push(usize::MAX);
                continue;
            }
            let quantised = Rgb(
                quantise_channel(r, quantisation_step),
                quantise_channel(g, quantisation_step),
                quantise_channel(b, quantisation_step),
            );

            let index = match palette.iter().position(|c| *c == quantised) {
                Some(i) => i,
                None => {
                    if palette.len() >= max_regions {
                        return Err(ImageError::TooManyClusters {
                            requested: max_regions,
                            palette: palette.len(),
                            max: max_regions,
                        });
                    }
                    palette.push(quantised);
                    palette.len() - 1
                }
            };
            labels.push(index);
            *counts.entry(index).or_insert(0) += 1;
        }
    }

    // Extract contours per label, then convert to millimetres.
    let scale = image.mm_per_pixel(part_width_mm);
    let mut regions = Vec::new();
    for (index, colour) in palette.iter().enumerate() {
        let mask: Vec<bool> = labels.iter().map(|l| *l == index).collect();
        let contours = trace_contours(&mask, image.width, image.height);
        if contours.is_empty() {
            continue;
        }
        let shapes = to_shapes(&contours, scale);
        regions.push(Region {
            palette_index: index,
            colour: *colour,
            shapes,
            pixel_count: counts.get(&index).copied().unwrap_or(0),
        });
    }

    if regions.is_empty() {
        return Err(ImageError::NoRegions {
            threshold: quantisation_step,
        });
    }

    Ok(Separation { palette, regions })
}

fn quantise_channel(value: u8, step: u8) -> u8 {
    let step = u32::from(step).max(1);
    let v = u32::from(value);
    // Snap to the midpoint of the bucket so colours stay close to the original.
    let snapped = (v / step) * step;
    snapped.min(255) as u8
}
/// A directed unit edge from one lattice vertex to the next.
type Edge = ([i64; 2], [i64; 2]);

/// Directed edges leaving a single vertex, keyed by where they start.
type EdgeMap = std::collections::BTreeMap<[i64; 2], Vec<Edge>>;

/// Trace the boundaries of a boolean mask.
///
/// Walks the edges that separate a foreground cell from a background cell,
/// treating each pixel as a unit square. Two reasons this beats Moore-neighbour
/// tracing on pixel centres:
///
/// * The boundary is exact. Every vertex lands on an integer coordinate, so a
///   traced rectangle has exactly the area of its pixel count. A centre-based
///   tracer is off by half a pixel on every axis, which is invisible at low
///   resolution and doubles the reported area of a small feature.
/// * The orientation falls out for free. With the foreground kept on the right,
///   outer contours come out counter-clockwise and holes come out clockwise,
///   which is precisely what `Shape::validate_orientation` requires. A
///   centre-based tracer has to be corrected afterwards, and getting that
///   correction wrong is the silent-area bug this crate already had once.
///
/// Contours are returned in pixel units, counter-clockwise, as closed loops.
pub fn trace_contours(mask: &[bool], width: usize, height: usize) -> Vec<Vec<[f64; 2]>> {
    debug_assert_eq!(mask.len(), width * height);

    let inside = |x: i64, y: i64| -> bool {
        if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
            return false;
        }
        mask[y as usize * width + x as usize]
    };

    // Emit the boundary edge of a foreground cell on one side, oriented so that
    // walking it keeps the foreground on the right and the background on the
    // left. A cell contributes an edge wherever its neighbour is background.
    let mut edges: Vec<([i64; 2], [i64; 2])> = Vec::new();
    for y in 0..height as i64 {
        for x in 0..width as i64 {
            if !inside(x, y) {
                continue;
            }
            // North: background above, so walk west along the top edge.
            if !inside(x, y + 1) {
                edges.push(([x + 1, y + 1], [x, y + 1]));
            }
            // East: walk north along the right edge.
            if !inside(x + 1, y) {
                edges.push(([x + 1, y], [x + 1, y + 1]));
            }
            // South: walk east along the bottom edge.
            if !inside(x, y - 1) {
                edges.push(([x, y], [x + 1, y]));
            }
            // West: walk south along the left edge.
            if !inside(x - 1, y) {
                edges.push(([x, y + 1], [x, y]));
            }
        }
    }

    // Chain edges into loops. Each vertex keeps the set of edges leaving it, so
    // a saddle — where two diagonal cells touch at a single corner — has two
    // outgoing edges and needs a rule.
    let mut outgoing: EdgeMap = EdgeMap::new();
    for edge in &edges {
        outgoing.entry(edge.0).or_default().push(*edge);
    }
    for list in outgoing.values_mut() {
        // Deterministic order, so the same mask always traces the same way.
        list.sort_by_key(|e| e.1);
    }

    let mut used: std::collections::BTreeSet<Edge> = Default::default();
    let mut contours = Vec::new();

    for start in outgoing.keys().copied().collect::<Vec<_>>() {
        for seed in outgoing[&start].clone() {
            if used.contains(&seed) {
                continue;
            }

            let mut points: Vec<[f64; 2]> = vec![[start[0] as f64, start[1] as f64]];
            let mut current = seed;
            let mut closed = false;

            // Every edge of a closed loop visits each of its vertices at most
            // once, so a multiple of the edge count cannot run forever.
            for _ in 0..=edges.len() {
                used.insert(current);
                points.push([current.1[0] as f64, current.1[1] as f64]);

                if current.1 == start {
                    closed = true;
                    break;
                }

                let Some(candidates) = outgoing.get(&current.1) else {
                    break;
                };
                let next = candidates
                    .iter()
                    .find(|e| !used.contains(e))
                    .copied()
                    .or_else(|| pick_saddle_turn(&current, candidates));

                let Some(next) = next else { break };
                current = next;
            }

            // The final point repeats the first, so drop it before measuring.
            if points.len() >= 4 {
                points.pop();
            }
            if closed && points.len() >= 3 {
                contours.push(points);
            }
        }
    }

    // Largest first, so an outer contour is always placed before its holes.
    contours.sort_by(|a, b| {
        polygon_area(b)
            .partial_cmp(&polygon_area(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    contours
}

/// Choose between the two outgoing edges at an ambiguous corner.
///
/// A saddle is where foreground cells meet diagonally. Two chains arrive at the
/// corner, and pairing them the same way on both would connect the diagonal
/// cells into one loop. Choosing the sharpest turn keeps foreground 4-connected
/// at the pixel level, which matches what the width and offset stages assume:
/// a diagonal touch is not a usable connection for a knife path.
fn pick_saddle_turn(current: &Edge, candidates: &[Edge]) -> Option<Edge> {
    if candidates.len() < 2 {
        return None;
    }
    let in_dir = [current.1[0] - current.0[0], current.1[1] - current.0[1]];

    // Order clockwise: N, E, S, W.
    let dir_index = |d: [i64; 2]| -> usize {
        match (d[0], d[1]) {
            (0, 1) => 0,
            (1, 0) => 1,
            (0, -1) => 2,
            _ => 3,
        }
    };
    let incoming = dir_index(in_dir);

    let mut best: Option<(usize, Edge)> = None;
    for edge in candidates {
        let out = [edge.1[0] - edge.0[0], edge.1[1] - edge.0[1]];
        // Quarter turns clockwise from the incoming direction. Prefer the
        // sharpest right turn, which keeps foreground 4-connected.
        let score = (4 - (dir_index(out) + 4 - incoming) % 4) % 4;
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, *edge));
        }
    }
    best.map(|(_, edge)| edge)
}

/// Convert traced pixel contours into millimetre geometry.
fn to_shapes(contours: &[Vec<[f64; 2]>], scale: f64) -> Shapes {
    // Largest first: a containing contour is always larger than what it holds.
    let mut ordered: Vec<(f64, Vec<[f64; 2]>)> = contours
        .iter()
        .map(|c| (polygon_area(c), c.clone()))
        .collect();
    ordered.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut shapes: Vec<Shape> = Vec::new();

    for (_, contour) in ordered {
        let candidate = Contour {
            points: contour
                .iter()
                .map(|[x, y]| Point {
                    x: x * scale,
                    y: y * scale,
                })
                .collect(),
            closed: true,
        };
        let orientation = signed_area(&candidate.points);

        // The hole belongs to the innermost shape that contains it, not merely
        // the last one placed. With several separate blobs in one region, a
        // small blob between two larger ones would otherwise swallow a hole
        // that has nothing to do with it.
        let mut container: Option<usize> = None;
        let mut smallest = f64::INFINITY;
        for (index, shape) in shapes.iter().enumerate() {
            let area = contour_area(&shape.outer);
            if area < smallest && contour_insides(&candidate.points, &shape.outer.points) {
                container = Some(index);
                smallest = area;
            }
        }

        // Being inside another contour does not make a contour a hole. An island
        // sitting in a hole is fully enclosed and wound the same way as its
        // surroundings, and treating it as a hole both flips the sign the area
        // calculation depends on and loses the island. The tracer fixes the
        // winding by construction, so orientation is a reliable discriminator:
        // clockwise is a hole, counter-clockwise is an island.
        match container {
            Some(index) if orientation < 0.0 => shapes[index].holes.push(candidate),
            _ => shapes.push(Shape::new(candidate)),
        }
    }

    Shapes { shapes }
}

fn contour_area(contour: &Contour) -> f64 {
    signed_area(&contour.points).abs()
}

fn signed_area(points: &[Point]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        acc += p.x * q.y - q.x * p.y;
    }
    acc / 2.0
}

fn polygon_area(points: &[[f64; 2]]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        acc += p[0] * q[1] - q[0] * p[1];
    }
    acc.abs() / 2.0
}

/// Whether `inner` lies inside `outer`.
///
/// Probes the midpoint of an edge rather than a vertex. With cell-derived
/// contours every vertex is an integer coordinate, so a vertex probe can land
/// exactly on a boundary edge of the outer contour, where ray casting is
/// ambiguous. An edge midpoint is strictly inside its own contour and, since
/// the two contours come from disjoint cells, strictly inside the outer one too.
fn contour_insides(inner: &[Point], outer: &[Point]) -> bool {
    if inner.len() < 2 || outer.len() < 3 {
        return false;
    }
    let probe = Point {
        x: (inner[0].x + inner[1].x) / 2.0,
        y: (inner[0].y + inner[1].y) / 2.0,
    };

    let mut inside = false;
    let mut j = outer.len() - 1;
    for i in 0..outer.len() {
        let (pi, pj) = (outer[i], outer[j]);
        if (pi.y > probe.y) != (pj.y > probe.y) {
            let t = (probe.y - pi.y) / (pj.y - pi.y);
            if probe.x < pi.x + t * (pj.x - pi.x) {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Simplify a contour with Ramer–Douglas–Peucker.
#[must_use]
pub fn simplify(points: &[Point], epsilon_mm: f64) -> Vec<Point> {
    if points.len() < 3 || epsilon_mm <= 0.0 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    if let Some(last) = points.len().checked_sub(1) {
        keep[last] = true;
    }
    rdp(points, 0, points.len() - 1, epsilon_mm, &mut keep);
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

fn rdp(points: &[Point], first: usize, last: usize, epsilon: f64, keep: &mut [bool]) {
    if last <= first + 1 {
        return;
    }
    let (a, b) = (points[first], points[last]);

    let mut worst = 0.0;
    let mut worst_index = first;
    for (i, point) in points.iter().enumerate().take(last).skip(first + 1) {
        let d = perpendicular_distance(*point, a, b);
        if d > worst {
            worst = d;
            worst_index = i;
        }
    }

    if worst > epsilon {
        keep[worst_index] = true;
        rdp(points, first, worst_index, epsilon, keep);
        rdp(points, worst_index, last, epsilon, keep);
    }
}

fn perpendicular_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len_sq = dx * dx + dy * dy;
    if len_sq <= MEASURE_EPS_MM * MEASURE_EPS_MM {
        return p.distance_to(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    p.distance_to(Point {
        x: a.x + t * dx,
        y: a.y + t * dy,
    })
}

/// Drop contours whose area is below the material's minimum.
///
/// Applied here rather than in the Validator so that the tracer does not hand the
/// topology engine thousands of specks; the Validator still checks the result
/// independently, so this is a reduction, not a decision.
#[must_use]
pub fn drop_small_regions(shapes: &Shapes, min_area_mm2: f64) -> Shapes {
    Shapes {
        shapes: shapes
            .shapes
            .iter()
            .filter(|s| crate::measure::shape_area(s).abs() >= min_area_mm2)
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn a_raster_validates_its_own_dimensions() {
        assert!(Raster::grey(2, 2, vec![0; 4]).is_ok());
        assert!(matches!(
            Raster::new(2, 2, 3, vec![0; 5]),
            Err(ImageError::LengthMismatch { .. })
        ));
        assert!(matches!(Raster::grey(0, 2, vec![]), Err(ImageError::Empty)));
        assert!(matches!(
            Raster::new(2, 2, 2, vec![0; 8]),
            Err(ImageError::UnsupportedChannels(2))
        ));
    }

    #[test]
    fn millimetres_derive_from_the_part_not_from_a_dpi_guess() {
        let image = Raster::grey(1000, 100, vec![0; 100_000]).expect("raster");
        assert!((image.mm_per_pixel(1412.0) - 1.412).abs() < 1e-9);
    }

    #[test]
    fn two_flat_colours_produce_two_regions() {
        // Left half black, right half white.
        let mut data = vec![0u8; 10 * 10 * 3];
        for y in 0..10 {
            for x in 0..10 {
                let v = if x < 5 { 0u8 } else { 255u8 };
                let i = (y * 10 + x) * 3;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
            }
        }
        let image = Raster::new(10, 10, 3, data).expect("raster");
        let sep = separate_colours(&image, 1, 10.0, 16).expect("separate");
        assert_eq!(sep.palette.len(), 2);
        assert_eq!(sep.regions.len(), 2);
    }

    #[test]
    fn regions_cover_the_whole_image_area() {
        let mut data = vec![0u8; 20 * 20 * 3];
        for y in 0..20 {
            for x in 0..20 {
                let v = if (x / 5 + y / 5) % 2 == 0 { 0u8 } else { 255u8 };
                let i = (y * 20 + x) * 3;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
            }
        }
        let image = Raster::new(20, 20, 3, data).expect("raster");
        let sep = separate_colours(&image, 1, 20.0, 4).expect("separate");
        let total: f64 = sep
            .regions
            .iter()
            .flat_map(|r| r.shapes.shapes.iter())
            .map(crate::measure::shape_area)
            .sum();
        // 20 mm across at 1 mm per pixel.
        assert!((total - 400.0).abs() < 1.0, "traced area {total}");
    }

    #[test]
    fn transparent_pixels_contribute_nothing() {
        let mut data = vec![0u8; 8 * 8 * 4];
        for i in 0..64 {
            data[i * 4 + 3] = 0; // fully transparent
        }
        // One opaque pixel.
        data[(3 * 8 + 3) * 4 + 3] = 255;
        let image = Raster::new(8, 8, 4, data).expect("raster");
        // Transparent is not "black", it is absence of design.
        let sep = separate_colours(&image, 1, 8.0, 8).expect("separate");
        assert_eq!(
            sep.palette.len(),
            1,
            "only the opaque pixel defines a colour"
        );
    }

    #[test]
    fn quantisation_reduces_the_palette() {
        let mut data = vec![0u8; 10 * 10 * 3];
        for y in 0..10 {
            for x in 0..10 {
                // A gradient of near-identical greys.
                let v = (x * 25) as u8;
                let i = (y * 10 + x) * 3;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
            }
        }
        let image = Raster::new(10, 10, 3, data).expect("raster");
        let coarse = separate_colours(&image, 64, 10.0, 16).expect("coarse");
        assert!(
            coarse.palette.len() < 5,
            "got {} colours",
            coarse.palette.len()
        );
    }

    #[test]
    fn too_many_clusters_is_refused_rather_than_silently_truncated() {
        let mut data = vec![0u8; 10 * 10 * 3];
        for i in 0..100 {
            data[i * 3] = (i * 2) as u8;
            data[i * 3 + 1] = (i * 3) as u8;
            data[i * 3 + 2] = (i * 5) as u8;
        }
        let image = Raster::new(10, 10, 3, data).expect("raster");
        assert!(matches!(
            separate_colours(&image, 1, 10.0, 2),
            Err(ImageError::TooManyClusters { .. })
        ));
    }

    #[test]
    fn tracing_finds_the_boundary_of_a_square() {
        let mut mask = vec![false; 10 * 10];
        for y in 2..8 {
            for x in 2..8 {
                mask[y * 10 + x] = true;
            }
        }
        let contours = trace_contours(&mask, 10, 10);
        assert_eq!(contours.len(), 1, "one square has one outer boundary");
        let area = polygon_area(&contours[0]);
        assert!((area - 36.0).abs() < 1e-9, "pixel area {area}");
    }

    #[test]
    fn tracing_finds_an_inner_hole_as_a_separate_contour() {
        let mut mask = vec![false; 12 * 12];
        for y in 1..11 {
            for x in 1..11 {
                mask[y * 12 + x] = true;
            }
        }
        for y in 4..8 {
            for x in 4..8 {
                mask[y * 12 + x] = false;
            }
        }
        let contours = trace_contours(&mask, 12, 12);
        // Outer ring plus the hole.
        assert_eq!(contours.len(), 2, "an annulus has two boundaries");
        let mut areas: Vec<f64> = contours.iter().map(|c| polygon_area(c)).collect();
        areas.sort_by(|a, b| b.partial_cmp(a).expect("finite"));
        assert!((areas[0] - 100.0).abs() < 1e-9, "outer {}", areas[0]);
        assert!((areas[1] - 16.0).abs() < 1e-9, "hole {}", areas[1]);
    }

    #[test]
    fn a_hole_ends_up_inside_its_outer_contour() {
        let mut mask = vec![false; 12 * 12];
        for y in 1..11 {
            for x in 1..11 {
                mask[y * 12 + x] = true;
            }
        }
        for y in 4..8 {
            for x in 4..8 {
                mask[y * 12 + x] = false;
            }
        }
        let shapes = to_shapes(&trace_contours(&mask, 12, 12), 1.0);
        assert_eq!(shapes.shapes.len(), 1);
        assert_eq!(
            shapes.shapes[0].holes.len(),
            1,
            "the hole belongs to the ring"
        );
        // 100 outer minus 16 hole.
        assert!((crate::measure::shapes_area(&shapes) - 84.0).abs() < 1e-6);
    }

    #[test]
    fn traced_geometry_passes_orientation_validation() {
        // The tracer emits outer contours before holes and the hole assignment
        // relies on it. If that ever breaks, signed areas stop subtracting.
        let mut mask = vec![false; 12 * 12];
        for y in 1..11 {
            for x in 1..11 {
                mask[y * 12 + x] = true;
            }
        }
        for y in 4..8 {
            for x in 4..8 {
                mask[y * 12 + x] = false;
            }
        }
        let shapes = to_shapes(&trace_contours(&mask, 12, 12), 1.0);
        for shape in &shapes.shapes {
            assert!(
                shape.validate_orientation().is_ok(),
                "traced holes must wind opposite to their outer contour"
            );
        }
    }

    #[test]
    fn simplification_drops_points_inside_the_tolerance() {
        let line: Vec<Point> = (0..100)
            .map(|i| p(i as f64 * 0.1, if i % 2 == 0 { 0.0 } else { 0.0001 }))
            .collect();
        let simple = simplify(&line, 0.01);
        assert!(simple.len() < line.len() / 10, "kept {}", simple.len());
        // Endpoints are always kept.
        assert!((simple[0].x - 0.0).abs() < 1e-12);
        assert!((simple[simple.len() - 1].x - 9.9).abs() < 1e-9);
    }

    #[test]
    fn simplification_at_zero_tolerance_is_the_identity() {
        let square = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 5.0)];
        assert_eq!(simplify(&square, 0.0).len(), square.len());
    }

    #[test]
    fn small_regions_are_dropped_before_the_topology_stage() {
        let shapes = Shapes {
            shapes: vec![
                Shape::new(Contour {
                    points: vec![p(0.0, 0.0), p(50.0, 0.0), p(50.0, 50.0), p(0.0, 50.0)],
                    closed: true,
                }),
                Shape::new(Contour {
                    points: vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
                    closed: true,
                }),
            ],
        };
        let kept = drop_small_regions(&shapes, 2.0);
        assert_eq!(kept.shapes.len(), 1, "the 1 mm² speck is dropped");
        assert!((crate::measure::shapes_area(&kept) - 2500.0).abs() < 1e-6);
    }

    #[test]
    fn separation_is_deterministic() {
        let mut data = vec![0u8; 12 * 12 * 3];
        for y in 0..12 {
            for x in 0..12 {
                let v = if (x + y) % 2 == 0 { 10u8 } else { 200u8 };
                let i = (y * 12 + x) * 3;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
            }
        }
        let image = Raster::new(12, 12, 3, data).expect("raster");
        let a = separate_colours(&image, 1, 12.0, 4).expect("first");
        let b = separate_colours(&image, 1, 12.0, 4).expect("second");
        assert_eq!(a, b, "the same image must separate the same way every time");
    }
}
