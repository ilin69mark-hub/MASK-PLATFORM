//! Mask Engine · geometry kernel.
//!
//! Pure geometry: no I/O, no clock, no environment, no network. That constraint
//! is what lets the same code run on the server, in the browser through wasm32,
//! and in the geometry CLI (P-07, ADR-0013).
//!
//! Tolerances live in [`constants`] and are defined by ADR-0004. Nothing in this
//! crate may hard-code a numeric tolerance.

pub mod boolean;
pub mod bridge;
pub mod canonical;
pub mod constants;
pub mod geom;
pub mod hash;
pub mod measure;
pub mod offset;
pub mod raster;

pub use boolean::{difference, intersection, normalise, union, xor, OverlayError};
pub use canonical::{canonicalise, canonicalise_shapes, to_path_data};
pub use constants::{
    report, CANON_COORD_DP, CANON_QUANTUM_MM, COLLINEAR_MM, GEOM_EPS_MM, LIB_ENGINE_BITS,
    LIB_SCALE, MAX_COORDINATE_MM, MEASURE_EPS_MM, MEASURE_REPORT_DP, MIN_WIDTH_EPS_MM,
};
pub use geom::{Contour, GeometryError, Point, Shape, Shapes};
pub use hash::{hash_json, ContentHash, HASH_SCHEME_VERSION};
pub use measure::{
    bounding_box, contour_perimeter, meets_threshold, point_in_contour, shape_area, shape_bounds,
    shapes_area, signed_contour_area, unanchored_islands,
};
pub use offset::{lib_math_mode, min_width, offset_shape, survives_inward_offset, widen};
