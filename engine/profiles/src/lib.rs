//! Manufacturing profiles. Data, not code (P-05).
//!
//! Adding a material or a plotter must mean adding a record, never editing
//! code. Every numeric manufacturing limit in the system resolves back to a
//! field in this module.
//!
//! All lengths are millimetres.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Lifecycle of a profile version.
///
/// `Draft` means the limits came from a datasheet and have never been cut by
/// hand. A project using a draft profile **cannot** be approved
/// (MANUFACTURING_ENGINE.md §8), which is why the state exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileStatus {
    Draft,
    Active,
    Superseded,
    Deprecated,
}

impl ProfileStatus {
    /// Lowercase wire form, used in metadata and logs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Deprecated => "deprecated",
        }
    }

    #[must_use]
    pub fn is_physically_verified(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("cannot read profile {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot parse profile {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("profile {code} v{version} is inconsistent: {reason}")]
    Inconsistent {
        code: String,
        version: u32,
        reason: String,
    },
}

/// Minimum feature sizes the material can physically hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Minimums {
    pub line_width_mm: f64,
    pub gap_mm: f64,
    pub island_area_mm2: f64,
    pub island_width_mm: f64,
    pub feature_length_mm: f64,
    pub corner_radius_mm: f64,
    pub bridge_width_mm: f64,
    pub bridge_length_mm: f64,
}

/// Physical behaviour of the film.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Physical {
    pub adhesion: String,
    pub adhesive_system: String,
    pub peel_angle_min_deg: f64,
    pub peel_rate: String,
    pub max_temp_c: f64,
    pub tolerance_registration_mm: f64,
    #[serde(default)]
    pub surface_prep: Vec<String>,
}

/// Secondary limits that are not about minimum sizes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub max_density_per_100cm2: f64,
    pub min_angle_deg: f64,
    pub max_area_mm2: f64,
}

/// A material version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialProfile {
    pub code: String,
    pub version: u32,
    pub name: String,
    pub family: String,
    pub thickness_mm: f64,
    pub minimums: Minimums,
    pub physical: Physical,
    pub limits: Limits,
    pub status: ProfileStatus,
    #[serde(default)]
    pub verified_physical: bool,
    #[serde(default)]
    pub notes: String,
}

impl MaterialProfile {
    #[must_use]
    pub fn is_physically_verified(&self) -> bool {
        self.status.is_physically_verified() && self.verified_physical
    }

    /// Fail loudly on limits that would silently pass everything.
    pub fn validate(&self) -> Result<(), ProfileError> {
        let reason = |r: &str| ProfileError::Inconsistent {
            code: self.code.clone(),
            version: self.version,
            reason: r.to_string(),
        };

        if self.minimums.line_width_mm <= 0.0 {
            return Err(reason("minimum line width must be positive"));
        }
        if self.minimums.gap_mm <= 0.0 {
            return Err(reason("minimum gap must be positive"));
        }
        if self.minimums.corner_radius_mm > self.minimums.line_width_mm {
            return Err(reason(
                "minimum corner radius exceeds the minimum line width, which is unphysical",
            ));
        }
        if self.minimums.bridge_width_mm < self.minimums.line_width_mm {
            return Err(reason(
                "a bridge thinner than the minimum line width cannot hold",
            ));
        }
        if self.physical.tolerance_registration_mm < 0.0 {
            return Err(reason("registration tolerance must be non-negative"));
        }
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ProfileError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let profile: Self = serde_json::from_str(&text).map_err(|source| ProfileError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        profile.validate()?;
        Ok(profile)
    }
}

/// Blade geometry of the cutter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blade {
    pub angle_deg: f64,
    pub offset_mm: f64,
    pub kerf_mm: f64,
    pub curve_tolerance_mm: f64,
    /// Multiplier turning curve tolerance into a minimum corner radius.
    #[serde(default = "default_curve_multiplier")]
    pub curve_multiplier: f64,
}

fn default_curve_multiplier() -> f64 {
    4.0
}

/// Usable sheet area, excluding margins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkingArea {
    pub width_mm: f64,
    pub length_mm: f64,
    pub usable_margin_mm: f64,
}

/// A plotter version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlotterProfile {
    pub code: String,
    pub version: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub blade: Blade,
    pub working_area: WorkingArea,
    pub accuracy: Accuracy,
    pub status: ProfileStatus,
    #[serde(default)]
    pub verified_physical: bool,
}

/// Registration capability of the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accuracy {
    pub repeatability_mm: f64,
    pub registration_method: String,
    pub max_registration_error_mm: f64,
}

impl PlotterProfile {
    pub fn validate(&self) -> Result<(), ProfileError> {
        let reason = |r: &str| ProfileError::Inconsistent {
            code: self.code.clone(),
            version: self.version,
            reason: r.to_string(),
        };

        if self.blade.kerf_mm <= 0.0 {
            return Err(reason("kerf must be positive"));
        }
        if self.blade.curve_tolerance_mm <= 0.0 {
            return Err(reason("curve tolerance must be positive"));
        }
        if self.blade.curve_multiplier < 1.0 {
            return Err(reason(
                "a curve multiplier below 1 would loosen the corner radius",
            ));
        }
        if self.working_area.width_mm <= 0.0 || self.working_area.length_mm <= 0.0 {
            return Err(reason("working area must be positive"));
        }
        if self.working_area.usable_margin_mm < 0.0 {
            return Err(reason("margin must be non-negative"));
        }
        if self.working_area.width_mm <= 2.0 * self.working_area.usable_margin_mm {
            return Err(reason("margins consume the entire working width"));
        }
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ProfileError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let profile: Self = serde_json::from_str(&text).map_err(|source| ProfileError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        profile.validate()?;
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material() -> MaterialProfile {
        MaterialProfile {
            code: "matte-vinyl-090".into(),
            version: 4,
            name: "Matte vinyl 0.09 mm".into(),
            family: "vinyl".into(),
            thickness_mm: 0.09,
            minimums: Minimums {
                line_width_mm: 0.80,
                gap_mm: 0.90,
                island_area_mm2: 2.40,
                island_width_mm: 0.60,
                feature_length_mm: 3.00,
                corner_radius_mm: 0.40,
                bridge_width_mm: 1.20,
                bridge_length_mm: 25.0,
            },
            physical: Physical {
                adhesion: "medium".into(),
                adhesive_system: "water_sensitive".into(),
                peel_angle_min_deg: 120.0,
                peel_rate: "slow".into(),
                max_temp_c: 80.0,
                tolerance_registration_mm: 0.35,
                surface_prep: vec!["degrease".into(), "dry".into()],
            },
            limits: Limits {
                max_density_per_100cm2: 180.0,
                min_angle_deg: 30.0,
                max_area_mm2: 400_000.0,
            },
            status: ProfileStatus::Active,
            verified_physical: true,
            notes: String::new(),
        }
    }

    fn plotter() -> PlotterProfile {
        PlotterProfile {
            code: "knife-25mm".into(),
            version: 2,
            name: "Knife plotter 25 mm holder".into(),
            kind: "knife_plotter".into(),
            blade: Blade {
                angle_deg: 45.0,
                offset_mm: 0.10,
                kerf_mm: 0.15,
                curve_tolerance_mm: 0.08,
                curve_multiplier: 4.0,
            },
            working_area: WorkingArea {
                width_mm: 300.0,
                length_mm: 1200.0,
                usable_margin_mm: 8.0,
            },
            accuracy: Accuracy {
                repeatability_mm: 0.05,
                registration_method: "crosshair_marks".into(),
                max_registration_error_mm: 0.30,
            },
            status: ProfileStatus::Active,
            verified_physical: true,
        }
    }

    #[test]
    fn a_consistent_profile_validates() {
        material().validate().unwrap();
        plotter().validate().unwrap();
    }

    #[test]
    fn a_corner_radius_wider_than_the_line_is_rejected() {
        let mut m = material();
        m.minimums.corner_radius_mm = 2.0;
        assert!(m.validate().is_err(), "an unphysical profile must not load");
    }

    #[test]
    fn a_bridge_thinner_than_the_line_is_rejected() {
        let mut m = material();
        m.minimums.bridge_width_mm = 0.10;
        assert!(m.validate().is_err());
    }

    #[test]
    fn margins_consuming_the_whole_field_are_rejected() {
        let mut p = plotter();
        p.working_area.usable_margin_mm = 200.0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn a_curve_multiplier_below_one_is_rejected() {
        let mut p = plotter();
        p.blade.curve_multiplier = 0.5;
        assert!(p.validate().is_err());
    }

    #[test]
    fn draft_profiles_are_not_physically_verified() {
        let mut m = material();
        m.status = ProfileStatus::Draft;
        assert!(!m.is_physically_verified());
        m.status = ProfileStatus::Active;
        assert!(m.is_physically_verified());
    }

    #[test]
    fn active_status_without_a_physical_run_is_still_unverified() {
        let mut m = material();
        m.verified_physical = false;
        assert!(!m.is_physically_verified());
    }

    #[test]
    fn profiles_round_trip_through_json() {
        let original = material();
        let text = serde_json::to_string(&original).unwrap();
        let parsed: MaterialProfile = serde_json::from_str(&text).unwrap();
        assert_eq!(original, parsed);
    }
}
