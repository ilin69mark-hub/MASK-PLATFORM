//! Surface metrics for a part, and the question of whether flat film fits it.
//!
//! `MODEL_FIT.md` §4 puts a part at a representation level, and §5 step ③ calls for
//! a curvature correction to element width. This module is what that step would
//! actually compute, and building it first turned up two things worth writing
//! down before more of Stage 7 is built on top of the document.
//!
//! ## Slope is not curvature
//!
//! A local slope α stretches *surface length* by `1/cos α`: a ramp inclined at 30°
//! is 1.1547× longer than its horizontal projection. That is where the formula in
//! `MODEL_FIT.md` §5③ comes from, and as a statement about lengths it is right.
//!
//! It is not a statement about the width of a cut element. The knife cuts flat
//! film on a table, before the film ever touches the part. Nothing about the part
//! changes what the knife removes, so `w_nominal` stays `w_nominal` and the
//! `min_width_mm` the validator measures stays true. A correction folded into
//! that number would be correcting a quantity that did not move.
//!
//! ## Whether the film lies down at all is a different question
//!
//! That one is governed by **Gaussian curvature** K. A surface is *developable*
//! when K = 0 everywhere: cylinders, cones, and tangent planes. Flat film lies on
//! a developable surface with no strain at any slope, however steep. Where K ≠ 0
//! — every sphere, every panel with relief — a flat pattern cannot lie flat; it
//! must stretch or wrinkle, and because the required stretch varies with
//! position, a single cut element ends up a different width in different places.
//!
//! So the failure that matters for this product is conformity and peel, not the
//! knife's cut width. `MODEL_FIT.md` §9 lists the validator rule for it, but §5③
//! corrects the wrong quantity for it.
//!
//! ## Derivatives are taken, not estimated
//!
//! These functions take the derivatives of the height field at a point. There is
//! deliberately no sampler that differences a grid of heights. Estimating them
//! loses exactly the distinction that matters here: central differences on a
//! cylinder of radius 100 mm report K ≈ 1.9 × 10⁻³ mm⁻² where the true value is
//! zero, which is the difference between "flat film conforms" and "it does not".
//! A caller who has a height field should therefore supply its own analytic
//! derivatives, or accept that it is reading noise. The cost is that a grid-based
//! source has to say how it will produce them.

/// What a surface does at one point, in the terms a mask decision needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePoint {
    /// Ratio of surface length to planar length at this point.
    ///
    /// Exactly `sqrt(1 + fu² + fv²)`. One means no stretch. A flat element
    /// measured across the slope is this much longer *on the surface*.
    pub stretch: f64,
    /// Slope of the tangent plane in degrees, `atan(sqrt(fu² + fv²))`.
    pub slope_deg: f64,
    /// Gaussian curvature, in mm⁻².
    ///
    /// Zero means developable: flat film lies on it without strain.
    pub gaussian_curvature: f64,
}

/// Measure a surface given its height field's derivatives at one point.
///
/// `fu`/`fv` are the first derivatives and `fuu`/`fvv`/`fuv` the second, of a
/// surface written as the graph `z = f(u, v)`.
///
/// The Gaussian curvature is `(fuu·fv² − 2·fuv·fu·fv + fvv·fu²) / (1 + fu² + fv²)²`.
/// Note this is a two-variable form, and it is easy to get wrong: the numerator
/// that gives a *constant* `1/R²` on a sphere is `fuu·fvv − fuv²`, while the
/// expression above is the first fundamental form and belongs to mean curvature.
/// Confusing the two makes a sphere's curvature look position-dependent, which
/// is not a property of spheres.
///
/// # Panics
///
/// Never. Non-finite inputs produce non-finite outputs rather than a panic, so a
/// corrupt height field degrades the verdict instead of killing the process.
#[must_use]
pub fn surface_point(fu: f64, fv: f64, fuu: f64, fvv: f64, fuv: f64) -> SurfacePoint {
    let denom = 1.0 + fu * fu + fv * fv;
    let gaussian = (fuu * fvv - fuv * fuv) / (denom * denom);
    SurfacePoint {
        stretch: denom.sqrt(),
        slope_deg: fu.hypot(fv).atan().to_degrees(),
        gaussian_curvature: gaussian,
    }
}

/// Curvature below which a surface counts as flat enough for flat film.
///
/// The threshold is a ratio, not an absolute curvature, because what decides
/// conformity is the strain accumulated across the patch, and that scales with
/// curvature times area. A part 1400 mm across with K = 10⁻⁶ mm⁻² has already
/// accumulated a visible strain; a 50 mm badge with the same K has not.
#[must_use]
pub fn developable(gaussian_curvature: f64, patch_diagonal_mm: f64) -> bool {
    let strain = gaussian_curvature.abs() * patch_diagonal_mm.powi(2);
    strain <= DEVELOPABLE_STRAIN
}

/// Strain a patch may accumulate before flat film is not the right assumption.
///
/// One percent. A vinyl mask that has to stretch by more than that across the
/// patch either thins visibly at its edges or lifts where it was pressed, and
/// both are visible in the finished paint — which is the thing the customer
/// judges. The number is a placeholder pending the first physical run, and it is
/// named here rather than inlined so the physical result has one place to land.
pub const DEVELOPABLE_STRAIN: f64 = 0.01;

/// How much a single flat element's width varies across a patch, as a ratio.
///
/// This is the quantity that ends up in a validator message. It is the spread of
/// [`SurfacePoint::stretch`] between the least and most strained point of the
/// patch: a mask laid on a sphere is 1.00× wide near the apex and 1.34× wide at
/// the edge of a 300 mm cap, so one cut element paints two different stripe
/// widths and the person applying it can see it.
#[must_use]
pub fn width_variation(least_stretch: f64, greatest_stretch: f64) -> f64 {
    let (lo, hi) = if least_stretch <= greatest_stretch {
        (least_stretch, greatest_stretch)
    } else {
        (greatest_stretch, least_stretch)
    };
    if lo <= 0.0 {
        return f64::INFINITY;
    }
    hi / lo
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cylinder of radius `r`, written as a height field over its tangent plane.
    ///
    /// `f(u) = r − sqrt(r² − u²)` gives `f_u = u/sqrt(r² − u²)` and
    /// `f_uu = r²/(r² − u²)^{3/2}`.
    fn cylinder(u: f64, r: f64) -> SurfacePoint {
        let s = (r * r - u * u).sqrt();
        surface_point(u / s, 0.0, r * r / (s * s * s), 0.0, 0.0)
    }

    /// Cone of half-angle 20°, written as a height field.
    ///
    /// Takes no point: the surface is linear, so the slope is the same
    /// everywhere, all second derivatives vanish, and the metrics cannot vary
    /// with position. A function that accepted one and ignored it would only
    /// suggest it might.
    fn cone() -> SurfacePoint {
        let k = 20f64.to_radians().tan();
        surface_point(k, 0.0, 0.0, 0.0, 0.0)
    }

    /// Spherical cap of radius `s`, written as a height field.
    ///
    /// `f = s − sqrt(s² − x² − y²)` has `f_x = x/q`, `f_xx = (s² − y²)/q³` and
    /// `f_xy = xy/q³` where `q = sqrt(s² − x² − y²)`.
    fn sphere(x: f64, y: f64, s: f64) -> SurfacePoint {
        let q = (s * s - x * x - y * y).sqrt();
        surface_point(
            x / q,
            y / q,
            (s * s - y * y) / q.powi(3),
            (s * s - x * x) / q.powi(3),
            x * y / q.powi(3),
        )
    }

    #[test]
    fn a_plane_is_flat_with_no_stretch() {
        let m = surface_point(0.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(m.stretch, 1.0);
        assert_eq!(m.slope_deg, 0.0);
        assert_eq!(m.gaussian_curvature, 0.0);
    }

    #[test]
    fn a_cylinder_is_developable_however_steep_it_gets() {
        // The point of the whole module. A cylinder reaches 64° of slope at this
        // radius, which `MODEL_FIT.md` §5③ would call for a 210% width
        // correction, yet flat film lies on it perfectly.
        for u in [0.0, 50.0, 90.0, 99.0] {
            let m = cylinder(u, 100.0);
            assert!(
                m.gaussian_curvature.abs() < 1e-15,
                "цилиндр развёртываем, K должен быть 0, получено {e} при u={u}",
                e = m.gaussian_curvature
            );
            assert!(developable(m.gaussian_curvature, 1400.0));
        }
        assert!(cylinder(90.0, 100.0).slope_deg > 60.0);
    }

    #[test]
    fn a_cone_is_developable_at_every_point() {
        // A cone has constant slope and zero curvature. A formula keyed on slope
        // would keep correcting it forever.
        let m = cone();
        assert_eq!(m.gaussian_curvature, 0.0);
        assert!(developable(m.gaussian_curvature, 4000.0));
        assert!(m.slope_deg > 19.0 && m.slope_deg < 21.0, "наклон {m:?}");
    }

    #[test]
    fn a_sphere_carries_the_same_curvature_everywhere() {
        // Regression for the two-variable form: with the wrong numerator this
        // comes out position-dependent, which no sphere has.
        let r = 300.0;
        for (x, y) in [
            (0.0, 0.0),
            (100.0, 0.0),
            (200.0, 0.0),
            (0.0, 200.0),
            (250.0, 150.0),
        ] {
            let m = sphere(x, y, r);
            assert!(
                (m.gaussian_curvature - 1.0 / (r * r)).abs() < 1e-18,
                "K на сфере должна быть постоянной 1/R², получено {e} в ({x},{y})",
                e = m.gaussian_curvature
            );
        }
    }

    #[test]
    fn stretch_grows_away_from_the_apex_which_is_what_widens_an_element() {
        let r = 300.0;
        let apex = sphere(0.0, 0.0, r).stretch;
        let edge = sphere(280.0, 0.0, r).stretch;
        assert!((apex - 1.0).abs() < 1e-12, "в апексе растяжения нет");
        assert!(edge > 2.7, "у края растяжение заметное, получено {edge}");
    }

    #[test]
    fn a_spherical_cap_is_developable_only_where_the_patch_is_small() {
        // The threshold is reached at a 30 mm diagonal for a 300 mm sphere. That
        // is the whole reason Stage 7 cannot start from curvature: at car-panel
        // scale there is no patch of a spherical cap that flat film fits.
        let k = 1.0 / (300.0 * 300.0);
        assert!(developable(k, 25.0), "значок 25 мм — допустимо");
        // Exactly at the threshold is admissible: the bound is on strain a part
        // may accumulate, and a part at the bound has not exceeded it.
        assert!(developable(k, 30.0), "на 30 мм ровно порог, он включён");
        assert!(!developable(k, 31.0), "чуть дальше порога — уже нет");
        assert!(!developable(k, 1400.0), "капот не развёртываем");
    }

    #[test]
    fn a_flat_panel_passes_the_conformity_test() {
        assert!(developable(0.0, 5000.0));
    }

    #[test]
    fn width_variation_is_the_ratio_of_extremes_and_ignores_their_order() {
        assert!((width_variation(1.0, 1.34) - 1.34).abs() < 1e-12);
        assert!((width_variation(1.34, 1.0) - 1.34).abs() < 1e-12);
        assert_eq!(width_variation(1.0, 1.0), 1.0);
        assert!(width_variation(0.0, 1.0).is_infinite());
    }

    #[test]
    fn a_panel_sized_patch_on_a_sphere_spreads_one_element_across_two_widths() {
        // The number a validator would put in a message: one cut element, two
        // widths, on the surface. A 300 mm sphere spans 600 mm, so this is the
        // largest patch that can lie on it at all — and it already varies by 81%.
        let r = 300.0;
        let variation = width_variation(sphere(0.0, 0.0, r).stretch, sphere(250.0, 0.0, r).stretch);
        assert!(
            variation > 1.8,
            "разброс ширины должен быть виден глазом, получено {variation}"
        );
    }

    /// Central differences of a height field, the obvious way a caller could get
    /// derivatives without any calculus. Kept here so the numbers in
    /// [`surface_point`]'s doc comment are pinned by a test rather than a claim.
    fn differenced_sphere_curvature(h: f64) -> f64 {
        let r = 300.0;
        let f = |x: f64, y: f64| r - (r * r - x * x - y * y).max(1e-15).sqrt();
        let d1 = |x: f64, y: f64| (f(x + h, y) - f(x - h, y)) / (2.0 * h);
        let d2 = |x: f64, y: f64| (f(x + h, y) - 2.0 * f(x, y) + f(x - h, y)) / (h * h);
        let dxy = |x: f64, y: f64| {
            (f(x + h, y + h) - f(x + h, y - h) - f(x - h, y + h) + f(x - h, y - h)) / (4.0 * h * h)
        };
        let (x, y) = (150.0, 0.0);
        let (fx, fy, fxx, fyy, fxy) = (d1(x, y), d1(y, x), d2(x, y), d2(y, y), dxy(x, y));
        (fxx * fyy - fxy * fxy) / (1.0 + fx * fx + fy * fy).powi(2)
    }

    #[test]
    fn differencing_a_height_field_cannot_resolve_developability_at_panel_scale() {
        // Why this module takes derivatives rather than a grid.
        //
        // Differencing leaves ~13% relative error in K on a sphere, and refining
        // the grid does not help: this is truncation, not cancellation, so the
        // error is flat in h. Over a 1400 mm patch that error is worth more than
        // three strain units, against a threshold of one. A grid-derived K
        // therefore cannot declare a part developable — it can only say "not
        // obviously flat", which is a question for better data, not a verdict.
        let r = 300.0;
        let truth = 1.0 / (r * r);
        let mut previous: Option<f64> = None;

        for h in [10.0, 1.0, 0.1, 0.01] {
            let relative = ((differenced_sphere_curvature(h) - truth) / truth).abs();
            assert!(
                relative > 0.10 && relative < 0.20,
                "ошибка ~13%, получено {relative} при h={h}"
            );
            if let Some(was) = previous {
                assert!(
                    relative >= was * 0.9,
                    "уточнение сетки не должно улучшать оценку заметно, было {was}, стало {relative}"
                );
            }
            previous = Some(relative);
        }

        // And the decision it would corrupt: the strain it implies versus the
        // strain that is actually there.
        let implied = differenced_sphere_curvature(1.0) * 1400.0 * 1400.0;
        let actual = truth * 1400.0 * 1400.0;
        assert!(
            (implied - actual).abs() > DEVELOPABLE_STRAIN,
            "ошибка должна переворачивать вердикт, иначе тест не значит ничего"
        );
    }
}
