//! Named tolerances. Single source of truth, defined by ADR-0004.
//!
//! Nothing in the kernel may hard-code a numeric tolerance. If a value is not
//! listed here, either it belongs here or it belongs in profile data.

/// Quantisation grid used for `content_hash` and for written coordinates.
pub const CANON_QUANTUM_MM: f64 = 1e-4;

/// Decimal places for coordinates when writing files (ADR-0004 §6).
pub const CANON_COORD_DP: usize = 4;

/// Decimal places for *reported* measurements, i.e. 1 micron (ADR-0004 §6).
pub const MEASURE_REPORT_DP: usize = 3;

/// Comparison epsilon for area and length assertions, in the same unit.
pub const MEASURE_EPS_MM: f64 = 1e-6;

/// Internal predicate epsilon: point-on-line, touching, self-intersection.
pub const GEOM_EPS_MM: f64 = 1e-9;

/// Collinearity / duplicate-point threshold used by canonicalisation.
pub const COLLINEAR_MM: f64 = 1e-7;

/// Comparison band when checking a measurement against a profile threshold.
///
/// An element 0.7995 mm wide against a 0.80 mm threshold passes, because
/// 0.7995 >= 0.80 - 0.001. Without this band the verdict on a boundary value
/// would depend on floating-point noise.
pub const MIN_WIDTH_EPS_MM: f64 = 1e-3;

/// Maximum plausible coordinate in millimetres.
///
/// ADR-0003 finding N-3: the geometry library accepts absurd coordinates
/// (1e12 mm tested), so the kernel validates bounds itself. The largest part
/// we model is a bonnet at ~1.5 m; this leaves two orders of headroom for
/// oversized nesting tiles while still rejecting nonsense.
pub const MAX_COORDINATE_MM: f64 = 100_000.0;

/// Integer engine used at the library boundary (ADR-0004 §1b, amendment).
pub const LIB_ENGINE_BITS: u32 = 64;

/// Float-to-integer scale at the library boundary: `1 / CANON_QUANTUM_MM`.
pub const LIB_SCALE: f64 = 1.0 / CANON_QUANTUM_MM;

/// Report a measurement rounded for human consumption (ADR-0004 §6).
#[must_use]
pub fn report(value: f64) -> f64 {
    round_to(value, MEASURE_REPORT_DP)
}

/// Round half away from zero to `places` decimal places.
///
/// Applied exactly once, at the presentation boundary. Computations never
/// round; rounding during computation would accumulate error.
#[must_use]
pub fn round_to(value: f64, places: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let factor = 10f64.powi(places as i32);
    let scaled = value * factor;
    // `round` rounds half away from zero, which is what we want and is
    // deterministic for both signs.
    scaled.round() / factor
}

/// Quantise a length onto the canonical grid.
///
/// Applied at serialisation time only, never during computation.
#[must_use]
pub fn quantise(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    round_to(value, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_uses_three_decimals() {
        assert!((report(0.799_500_1) - 0.8).abs() < 1e-12);
        assert!((report(0.42) - 0.42).abs() < 1e-12);
    }

    #[test]
    fn round_is_half_away_from_zero() {
        assert_eq!(round_to(0.000_05, 4), 0.000_1);
        assert_eq!(round_to(-0.000_05, 4), -0.000_1);
        assert_eq!(round_to(1.234_5, 3), 1.235);
    }

    #[test]
    fn quantise_snaps_onto_the_grid() {
        // 1/3 mm must not survive into a hash as 0.3333333333.
        let q = quantise(1.0 / 3.0);
        assert_eq!(q, 0.3333);
    }

    #[test]
    fn non_finite_passes_through_untouched() {
        // Callers must reject these explicitly; rounding must not silently
        // turn NaN into a number.
        assert!(quantise(f64::NAN).is_nan());
        assert!(report(f64::INFINITY).is_infinite());
    }

    #[test]
    fn threshold_band_accepts_sub_limit_by_one_band() {
        let threshold = 0.80;
        let width = 0.7995;
        assert!(width >= threshold - MIN_WIDTH_EPS_MM);
    }
}
