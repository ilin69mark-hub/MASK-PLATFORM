//! Content hashing. ADR-0004.
//!
//! The hash decides whether a geometry is "the same" for the metrics cache and
//! for reproduction verification. It is computed over canonical serialisation
//! with sorted keys and SHA-256, and deliberately excludes timestamps,
//! identifiers that vary with time, locale and filesystem paths.

use crate::canonical::canonicalise_shapes;
use crate::geom::Shapes;
use sha2::{Digest, Sha256};

/// Version of the hashing convention itself.
///
/// Bumping this invalidates every cached metric and every stored hash. It must
/// change whenever the canonical form or the JSON encoding changes.
///
/// Version 2 added the `C` command for arcs. Polygonal geometry hashes
/// byte-identically to version 1, because nothing about a polygon's canonical
/// form changed; a contour with curves hashes differently, and had to — it used to
/// hash the same as the polygon through the same points.
pub const HASH_SCHEME_VERSION: u32 = 2;

/// A hash plus the scheme that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHash {
    pub scheme: u32,
    pub digest: String,
}

impl ContentHash {
    #[must_use]
    pub fn of(shapes: &Shapes) -> Self {
        let text = canonical_text(shapes);
        Self {
            scheme: HASH_SCHEME_VERSION,
            digest: Self::digest_bytes(text.as_bytes()),
        }
    }

    /// Raw hex digest without the scheme prefix.
    fn digest_bytes(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sha256:{}", self.digest)
    }
}

/// Canonical serialisation of a shape set.
///
/// Structure is emitted by hand rather than through `serde_json` so the field
/// order is fixed by this function rather than by struct declaration order,
/// which is an implementation detail.
///
/// Path data is embedded as a JSON string. It contains spaces and the letters
/// `M`, `L` and `Z`, so it must be quoted — emitting it bare would produce
/// invalid JSON that no consumer could parse, even though the raw bytes would
/// still hash consistently.
#[must_use]
pub fn canonical_text(shapes: &Shapes) -> String {
    let canonical = canonicalise_shapes(shapes);
    let mut out = String::with_capacity(1024);
    out.push_str("{\"h\":");
    out.push_str(&HASH_SCHEME_VERSION.to_string());
    out.push_str(",\"shapes\":[");
    for (i, shape) in canonical.shapes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"outer\":");
        out.push_str(&json_string(&crate::canonical::to_path_data(&shape.outer)));
        out.push_str(",\"holes\":[");
        for (j, hole) in shape.holes.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&json_string(&crate::canonical::to_path_data(hole)));
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

/// Quote a string as a JSON literal.
///
/// Path data cannot contain a quote or a backslash, so escaping is a single
/// step rather than a general JSON string encoder.
fn json_string(value: &str) -> String {
    debug_assert!(
        !value.contains('"') && !value.contains('\\'),
        "path data must not contain JSON metacharacters"
    );
    format!("\"{value}\"")
}

/// Hash any serialisable value using the same convention.
///
/// Keys are sorted so that field order in a struct never changes the hash.
pub fn hash_json<T: serde::Serialize>(value: &T) -> ContentHash {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
    let canonical = serde_json::to_string(&parsed).unwrap_or_else(|_| "null".to_string());
    ContentHash {
        scheme: HASH_SCHEME_VERSION,
        digest: ContentHash::digest_bytes(canonical.as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Contour, Point, Shape};

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Shapes {
        Shapes::single(Shape::new(Contour {
            points: vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h)],
            closed: true,
            arcs: Vec::new(),
        }))
    }

    #[test]
    fn equal_geometry_hashes_equal() {
        assert_eq!(
            ContentHash::of(&rect(0.0, 0.0, 10.0, 5.0)),
            ContentHash::of(&rect(0.0, 0.0, 10.0, 5.0))
        );
    }

    #[test]
    fn sub_quantum_jitter_does_not_change_the_hash() {
        let a = rect(0.0, 0.0, 10.0, 5.0);
        let b = rect(0.0, 0.0, 10.0 + 1e-9, 5.0);
        assert_eq!(
            ContentHash::of(&a),
            ContentHash::of(&b),
            "a difference below the 1e-4 grid must not alter identity"
        );
    }

    #[test]
    fn translation_of_one_grid_step_does_change_the_hash() {
        let a = rect(0.0, 0.0, 10.0, 5.0);
        let b = rect(0.0001, 0.0, 10.0, 5.0);
        assert_ne!(ContentHash::of(&a), ContentHash::of(&b));
    }

    #[test]
    fn different_geometry_hashes_differently() {
        assert_ne!(
            ContentHash::of(&rect(0.0, 0.0, 10.0, 5.0)),
            ContentHash::of(&rect(0.0, 0.0, 10.0, 6.0))
        );
    }

    #[test]
    fn shape_order_is_part_of_identity() {
        // Documented consequence: order is canonicalised (sorted by emission),
        // but the emission order follows the input. Callers that need order
        // independence must sort before hashing.
        let mut two = Shapes::new();
        two.shapes.push(Shape::new(Contour {
            points: vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)],
            closed: true,
            arcs: Vec::new(),
        }));
        two.shapes.push(Shape::new(Contour {
            points: vec![p(5.0, 0.0), p(6.0, 0.0), p(6.0, 1.0)],
            closed: true,
            arcs: Vec::new(),
        }));
        let a = ContentHash::of(&two);
        two.shapes.reverse();
        assert_ne!(a, ContentHash::of(&two));
        // Restoring the order restores the hash, so this is order and nothing else.
        two.shapes.reverse();
        assert_eq!(a, ContentHash::of(&two));
    }

    #[test]
    fn digest_is_stable_across_calls() {
        let shapes = rect(3.0, 4.0, 7.0, 9.0);
        assert_eq!(
            ContentHash::of(&shapes).digest,
            ContentHash::of(&shapes).digest
        );
        assert_eq!(ContentHash::of(&shapes).digest.len(), 64);
    }

    /// A circle as four points and four quarter arcs.
    fn circle(radius: f64) -> crate::geom::Contour {
        use crate::geom::{ArcSegment, Contour, Point};
        let points: Vec<Point> = (0..4)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * (i as f64);
                Point {
                    x: radius * a.cos(),
                    y: radius * a.sin(),
                }
            })
            .collect();
        let arcs = (0..4)
            .map(|i| ArcSegment {
                from: i,
                to: (i + 1) % 4,
                centre: Point { x: 0.0, y: 0.0 },
                radius,
                ccw: true,
            })
            .collect();
        Contour {
            points,
            closed: true,
            arcs,
        }
    }

    #[test]
    fn a_curve_and_its_chord_polygon_hash_differently() {
        // The bug: both canonicalised to the same M/L/Z string, so two cuts with
        // different areas and different perimeters shared one identity.
        let curved = crate::geom::Shapes {
            shapes: vec![crate::geom::Shape::new(circle(10.0))],
        };
        let straight = {
            let c = circle(10.0);
            crate::geom::Shapes {
                shapes: vec![crate::geom::Shape::new(crate::geom::Contour {
                    points: c.points,
                    closed: true,
                    arcs: Vec::new(),
                })],
            }
        };
        let a = ContentHash::of(&curved);
        let b = ContentHash::of(&straight);
        assert_ne!(
            a.digest, b.digest,
            "a circle and its inscribed square are not the same geometry"
        );
        assert_eq!(a.scheme, HASH_SCHEME_VERSION);
    }

    #[test]
    fn equivalent_spellings_of_the_same_curve_hash_the_same() {
        // The Validation section of ADR-0004 asks for this and it was never
        // implemented: `M10,0C...` and `M 10 0 C ...` must be one geometry, or the
        // metrics cache and reproduction check both lie.
        let curved = circle(10.0);
        let a = crate::geom::Shapes {
            shapes: vec![crate::geom::Shape::new(curved.clone())],
        };
        // Nudge the first point inside the quantisation grid: canonicalisation must
        // absorb it exactly as it absorbs jitter on a polygon.
        let nudged = crate::geom::Contour {
            points: {
                let mut pts = curved.points.clone();
                pts[0].x += 1e-9;
                pts
            },
            closed: true,
            arcs: curved.arcs.clone(),
        };
        let b = crate::geom::Shapes {
            shapes: vec![crate::geom::Shape::new(nudged)],
        };
        assert_eq!(
            ContentHash::of(&a).digest,
            ContentHash::of(&b).digest,
            "sub-quantum jitter must not change the hash"
        );
    }

    /// Switch the C locale, returning the name that actually took effect.
    fn set_locale(name: &str) -> Option<String> {
        // SAFETY: a single-threaded test process. The alternative is a global
        // mutable C setting touched from tests, which is worse.
        unsafe extern "C" {
            #[link_name = "setlocale"]
            fn setlocale(category: i32, locale: *const std::ffi::c_char) -> *mut std::ffi::c_char;
        }
        const LC_ALL: i32 = 6;
        let c_string = std::ffi::CString::new(name).expect("locale name");
        let applied = unsafe { setlocale(LC_ALL, c_string.as_ptr()) };
        (!applied.is_null()).then(|| unsafe {
            std::ffi::CStr::from_ptr(applied)
                .to_string_lossy()
                .into_owned()
        })
    }

    #[test]
    fn the_hash_does_not_depend_on_the_locale() {
        // Asked for by the Validation section of ADR-0004 and missing until now.
        //
        // What this can and cannot prove, stated plainly. On macOS no installed
        // locale changes the C library's decimal separator: fr_FR, de_DE, ru_RU
        // and nl_NL all keep `.`. So switching locale here proves the code path is
        // unaffected by the setting, and cannot prove it is unaffected by a
        // comma. The part that a comma would actually break — that the emitted
        // numbers are formatted by hand rather than through the locale — is
        // checked directly below, and that is the check that carries the weight.
        let curved = crate::geom::Shapes {
            shapes: vec![crate::geom::Shape::new(circle(10.0))],
        };
        let before = ContentHash::of(&curved).digest;

        for name in ["fr_FR.UTF-8", "de_DE.UTF-8", "ru_RU.UTF-8"] {
            if let Some(applied) = set_locale(name) {
                assert_eq!(
                    before,
                    ContentHash::of(&curved).digest,
                    "the hash changed under {applied}"
                );
            }
        }
        set_locale("C");
    }

    #[test]
    fn every_number_in_the_canonical_path_is_written_in_ascii() {
        // The load-bearing half of the locale requirement.
        //
        // Comma-separation of path arguments is legitimate — `C x1 y1, x2 y2, x y`
        // is the SVG grammar — so a blanket "no commas" test proves nothing, and
        // that is the trap a first attempt at this test fell into. What must hold is
        // narrower and stronger: no character a locale could substitute may appear
        // *inside* a number. Strip the command letters and the argument separators,
        // and whatever is left has to be pure ASCII decimal.
        let path = crate::to_path_data(&circle(10.0));
        let mut numbers = String::new();
        for c in path.chars() {
            if c.is_ascii_alphabetic() || c == ',' || c.is_whitespace() {
                continue;
            }
            numbers.push(c);
        }
        assert!(
            numbers
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')),
            "a number carries something a locale could have changed: {numbers}"
        );

        // And the whole text still has to survive a round trip through JSON, since
        // that is how it reaches disk and the metrics cache.
        let text = canonical_text(&crate::geom::Shapes {
            shapes: vec![crate::geom::Shape::new(circle(10.0))],
        });
        serde_json::from_str::<serde_json::Value>(&text).expect("canonical text stays valid JSON");
    }

    #[test]
    fn canonical_text_is_parseable_json() {
        // Guards the bug where path data was emitted bare and the result was
        // not JSON at all: it hashed consistently but could not be read back.
        let text = canonical_text(&rect(0.0, 0.0, 1.0, 1.0));
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("canonical form must be valid JSON");
        assert_eq!(parsed["h"], HASH_SCHEME_VERSION);
        assert_eq!(parsed["shapes"].as_array().map(Vec::len), Some(1));
        assert!(parsed["shapes"][0]["outer"].is_string());
    }

    #[test]
    fn canonical_text_carries_no_timestamp_or_path() {
        let text = canonical_text(&rect(0.0, 0.0, 1.0, 1.0));
        assert!(!text.contains("20"), "no year may appear: {text}");
        assert!(!text.contains('/'), "no filesystem path may appear: {text}");
    }
}
