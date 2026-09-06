//! The small level grammar that lets one stored request match on either
//! vocabulary.
//!
//! A request is stored as *(discipline, category, number, level type, value)*.
//! The parameter half is resolved by a
//! [`ParameterResolver`](crate::ParameterResolver); this is the level half. An
//! NCEP `.idx` renders a level as English — `2 m above ground`, `500 mb`,
//! `surface` — and an ECMWF `.index` as a `levtype` plus a `levelist` — `sfc`,
//! `pl` + `500`. Neither is the other's, so both are parsed into the same
//! [`LevelSpec`] and the query is written once.
//!
//! **What the grammar does not recognise is kept verbatim**, as
//! [`Surface::Named`], rather than dropped or guessed. There are around fifty
//! distinct level strings across the NCEP products, most of them named surfaces
//! with no value at all (`lowest level of the wet bulb zero`,
//! `PV=-1.5e-06 (Km^2/kg/s) surface`), and a grammar that tried to model each
//! one would be a worse copy of `wgrib2`'s own table. Naming them keeps the
//! string matchable and honest about not having been understood.

/// The surface a level sits on.
///
/// The modelled variants are the ones a stored request plausibly names. Anything
/// else is [`Self::Named`], which compares as the string the manifest wrote.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Surface {
    /// Ground or water surface.
    Surface,
    /// Mean sea level.
    MeanSeaLevel,
    /// An isobaric level, valued in hectopascals (`500 mb`, ECMWF `pl`).
    Isobaric,
    /// A pressure difference from the ground, in hectopascals
    /// (`30-0 mb above ground`).
    PressureAboveGround,
    /// A height above ground, in metres (`2 m above ground`).
    HeightAboveGround,
    /// A height above mean sea level, in metres.
    HeightAboveMeanSeaLevel,
    /// A depth below the land surface, in metres (`0-0.1 m below ground`,
    /// `0.5 m underground`).
    DepthBelowLand,
    /// A model hybrid level (ECMWF `ml`).
    HybridLevel,
    /// A sigma level or layer.
    SigmaLevel,
    /// An isentropic (potential-temperature) level, in kelvin (ECMWF `pt`).
    PotentialTemperature,
    /// An isotherm, in degrees Celsius (`0C isotherm`).
    Isotherm,
    /// The whole atmosphere as one layer.
    EntireAtmosphere,
    /// Cloud top.
    CloudTop,
    /// Cloud base.
    CloudBase,
    /// Cloud ceiling.
    CloudCeiling,
    /// The tropopause.
    Tropopause,
    /// The level of maximum wind.
    MaxWind,
    /// Top of the atmosphere (nominal).
    TopOfAtmosphere,
    /// The planetary boundary layer.
    PlanetaryBoundaryLayer,
    /// Anything the grammar does not model, as the manifest wrote it.
    Named(String),
}

/// A level, parsed as far as the grammar goes.
///
/// `value` and `value2` are in the surface's own unit — hectopascals for
/// [`Surface::Isobaric`], metres for the height and depth surfaces, kelvin for
/// [`Surface::PotentialTemperature`]. A layer between two bounds fills both, in
/// the order the manifest wrote them (`30-0 mb above ground` is
/// `value = 30`, `value2 = 0`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct LevelSpec {
    /// Which surface.
    pub surface: Surface,
    /// The level's value, or the layer's first bound.
    pub value: Option<f64>,
    /// The layer's second bound, when the level is a layer.
    pub value2: Option<f64>,
}

impl LevelSpec {
    /// A named surface with no value.
    pub fn named(surface: Surface) -> Self {
        Self {
            surface,
            value: None,
            value2: None,
        }
    }

    /// A single-valued level.
    pub fn at(surface: Surface, value: f64) -> Self {
        Self {
            surface,
            value: Some(value),
            value2: None,
        }
    }

    /// A layer between two bounds.
    pub fn between(surface: Surface, value: f64, value2: f64) -> Self {
        Self {
            surface,
            value: Some(value),
            value2: Some(value2),
        }
    }

    /// Whether `self`, read as a **query**, is satisfied by `candidate`.
    ///
    /// Asymmetric on purpose. The surfaces must be equal, but a query that
    /// states no value matches every value on that surface — "any isobaric
    /// level" is a request a user makes and "the isobaric level whose value is
    /// unstated" is not. A query that *does* state one requires it exactly:
    /// both sides come from decimal text through the same parse, so there is no
    /// accumulated error for a tolerance to absorb, and a tolerance would let
    /// `1000 mb` match `1000.5 mb` on a product that carries both.
    pub fn matches(&self, candidate: &LevelSpec) -> bool {
        if self.surface != candidate.surface {
            return false;
        }
        let bound_ok = |want: Option<f64>, have: Option<f64>| match want {
            None => true,
            Some(w) => have == Some(w),
        };
        bound_ok(self.value, candidate.value) && bound_ok(self.value2, candidate.value2)
    }
}

/// Named surfaces, in the exact wording NCEP's `.idx` uses.
///
/// Lower-cased on both sides before lookup, so `0C isotherm` and `NC isotherm`
/// reach the numeric path while `Cloud Top` and `cloud top` reach this one.
const NAMED: &[(&str, Surface)] = &[
    ("surface", Surface::Surface),
    ("mean sea level", Surface::MeanSeaLevel),
    ("entire atmosphere", Surface::EntireAtmosphere),
    // wgrib2 writes the long form on the products that inherited GRIB1's
    // level 200; both mean the same surface and a query must not have to know
    // which product it is reading.
    (
        "entire atmosphere (considered as a single layer)",
        Surface::EntireAtmosphere,
    ),
    ("cloud top", Surface::CloudTop),
    ("cloud base", Surface::CloudBase),
    ("cloud ceiling", Surface::CloudCeiling),
    ("tropopause", Surface::Tropopause),
    ("max wind", Surface::MaxWind),
    ("top of atmosphere", Surface::TopOfAtmosphere),
    ("planetary boundary layer", Surface::PlanetaryBoundaryLayer),
];

/// Unit phrases that follow a number, longest first.
///
/// Order is load-bearing: `m above mean sea level` has to be tried before
/// `m above ground` would ever be reached, and `mb above ground` before `mb`,
/// or a prefix would win and put the level on the wrong surface.
const UNITS: &[(&str, Surface)] = &[
    ("mb above ground", Surface::PressureAboveGround),
    ("m above mean sea level", Surface::HeightAboveMeanSeaLevel),
    ("m above ground", Surface::HeightAboveGround),
    ("m below ground", Surface::DepthBelowLand),
    ("m underground", Surface::DepthBelowLand),
    ("hybrid level", Surface::HybridLevel),
    ("sigma level", Surface::SigmaLevel),
    ("sigma layer", Surface::SigmaLevel),
    ("k level", Surface::PotentialTemperature),
    ("c isotherm", Surface::Isotherm),
    ("mb", Surface::Isobaric),
];

/// Parse a level string as an NCEP `.idx` renders it.
///
/// Never fails: an unrecognised string becomes [`Surface::Named`] carrying the
/// input verbatim, so nothing is lost and the string still matches a query
/// written against the same text.
pub fn parse_ncep_level(raw: &str) -> LevelSpec {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();

    if let Some((_, surface)) = NAMED.iter().find(|(name, _)| *name == lower) {
        return LevelSpec::named(surface.clone());
    }

    // A number, or a `a-b` pair, then a unit phrase. `0C isotherm` has no space
    // between the number and the unit, so the split is by where the number
    // stops rather than by whitespace.
    let (first, rest) = match split_number(&lower) {
        Some(pair) => pair,
        None => return LevelSpec::named(Surface::Named(trimmed.to_string())),
    };
    let (second, rest) = match rest.strip_prefix('-').and_then(split_number) {
        Some((v, r)) => (Some(v), r),
        None => (None, rest),
    };

    let unit = rest.trim_start();
    let Some((_, surface)) = UNITS.iter().find(|(name, _)| *name == unit) else {
        return LevelSpec::named(Surface::Named(trimmed.to_string()));
    };
    LevelSpec {
        surface: surface.clone(),
        value: Some(first),
        value2: second,
    }
}

/// Map an ECMWF `levtype` / `levelist` pair onto the same vocabulary.
///
/// `levelist` is absent for the single-level types, which is why it is an
/// `Option` rather than an empty string: `sfc` with no list is a surface, and
/// `pl` with no list is a request this crate cannot place and keeps named.
pub fn parse_ecmwf_level(levtype: &str, levelist: Option<&str>) -> LevelSpec {
    let value = levelist.and_then(|s| s.trim().parse::<f64>().ok());
    let surface = match levtype.trim().to_ascii_lowercase().as_str() {
        "sfc" => Surface::Surface,
        "pl" => Surface::Isobaric,
        "ml" => Surface::HybridLevel,
        "pt" => Surface::PotentialTemperature,
        // `pv` (potential vorticity) and anything newer keep their own tag, in
        // ECMWF's spelling, rather than being folded into a surface that means
        // something else.
        other => Surface::Named(other.to_string()),
    };
    // A `levelist` on a surface that takes no value would be a contradiction in
    // the sidecar; the surface wins and the value is dropped, because
    // `sfc` + `0` is how ECMWF writes "surface" and not "surface at zero".
    let value = match surface {
        Surface::Surface => None,
        _ => value,
    };
    LevelSpec {
        surface,
        value,
        value2: None,
    }
}

/// Split a leading decimal number off `s`, returning it and the rest.
///
/// Deliberately does **not** accept a leading `-`: the layer forms are written
/// `a-b`, so a minus can only ever be the separator, and accepting it here
/// would make `30-0 mb above ground` parse as the single value `30` followed by
/// the unit `0 mb above ground`.
fn split_number(s: &str) -> Option<(f64, &str)> {
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let value = s[..end].parse::<f64>().ok()?;
    Some((value, &s[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-valued families, from strings taken verbatim out of the
    /// committed NCEP sidecars.
    #[test]
    fn the_numeric_families_parse_to_a_surface_and_a_value() {
        assert_eq!(
            parse_ncep_level("2 m above ground"),
            LevelSpec::at(Surface::HeightAboveGround, 2.0)
        );
        assert_eq!(
            parse_ncep_level("500 mb"),
            LevelSpec::at(Surface::Isobaric, 500.0)
        );
        assert_eq!(
            parse_ncep_level("1 hybrid level"),
            LevelSpec::at(Surface::HybridLevel, 1.0)
        );
        assert_eq!(
            parse_ncep_level("0.995 sigma level"),
            LevelSpec::at(Surface::SigmaLevel, 0.995)
        );
        // No space between the number and the unit.
        assert_eq!(
            parse_ncep_level("0C isotherm"),
            LevelSpec::at(Surface::Isotherm, 0.0)
        );
        assert_eq!(
            parse_ncep_level("320 K level"),
            LevelSpec::at(Surface::PotentialTemperature, 320.0)
        );
    }

    /// A layer fills both bounds, in the order written.
    #[test]
    fn a_layer_keeps_both_bounds_in_order() {
        assert_eq!(
            parse_ncep_level("30-0 mb above ground"),
            LevelSpec::between(Surface::PressureAboveGround, 30.0, 0.0)
        );
        assert_eq!(
            parse_ncep_level("0-0.1 m below ground"),
            LevelSpec::between(Surface::DepthBelowLand, 0.0, 0.1)
        );
        assert_eq!(
            parse_ncep_level("6000-0 m above ground"),
            LevelSpec::between(Surface::HeightAboveGround, 6000.0, 0.0)
        );
    }

    /// `mb above ground` is a pressure *difference* from the ground, not an
    /// isobaric level. The longest-first unit order is what keeps them apart,
    /// and getting it wrong would put a boundary-layer field on the 30 hPa
    /// surface.
    #[test]
    fn a_pressure_layer_above_ground_is_not_isobaric() {
        let boundary = parse_ncep_level("30-0 mb above ground");
        assert_eq!(boundary.surface, Surface::PressureAboveGround);
        assert_ne!(boundary.surface, Surface::Isobaric);
        // …and the same holds for `m above mean sea level` against
        // `m above ground`.
        assert_eq!(
            parse_ncep_level("1000 m above mean sea level").surface,
            Surface::HeightAboveMeanSeaLevel
        );
    }

    /// Both spellings of the whole-column surface reach the same variant, so a
    /// query written against one product matches the other.
    #[test]
    fn the_two_spellings_of_entire_atmosphere_agree() {
        assert_eq!(
            parse_ncep_level("entire atmosphere"),
            parse_ncep_level("entire atmosphere (considered as a single layer)")
        );
    }

    /// Everything the grammar does not model survives verbatim, so it is still
    /// matchable and still says what the file said.
    #[test]
    fn an_unmodelled_level_is_kept_word_for_word() {
        for raw in [
            "PV=-1.5e-06 (Km^2/kg/s) surface",
            "lowest level of the wet bulb zero",
            "surface - 1372 m above ground",
            "reserved",
            "10 in sequence",
        ] {
            assert_eq!(
                parse_ncep_level(raw),
                LevelSpec::named(Surface::Named(raw.to_string())),
                "{raw}"
            );
        }
    }

    /// The ECMWF vocabulary lands on the same surfaces, which is the whole
    /// point: one stored request, two sidecar dialects.
    #[test]
    fn the_two_dialects_meet_on_one_surface() {
        assert_eq!(
            parse_ecmwf_level("pl", Some("500")),
            parse_ncep_level("500 mb")
        );
        assert_eq!(parse_ecmwf_level("sfc", None), parse_ncep_level("surface"));
        assert_eq!(
            parse_ecmwf_level("ml", Some("137")),
            parse_ncep_level("137 hybrid level")
        );
    }

    /// `sfc` carries no value even when the sidecar writes one.
    #[test]
    fn a_surface_levelist_does_not_become_a_value() {
        assert_eq!(
            parse_ecmwf_level("sfc", Some("0")),
            LevelSpec::named(Surface::Surface)
        );
    }

    /// A query that names only the surface matches every value on it; one that
    /// names a value does not match a different value. The asymmetry is the
    /// feature.
    #[test]
    fn a_valueless_query_matches_any_value_on_the_surface() {
        let any_isobaric = LevelSpec::named(Surface::Isobaric);
        assert!(any_isobaric.matches(&parse_ncep_level("500 mb")));
        assert!(any_isobaric.matches(&parse_ncep_level("850 mb")));
        assert!(!any_isobaric.matches(&parse_ncep_level("2 m above ground")));

        let five_hundred = LevelSpec::at(Surface::Isobaric, 500.0);
        assert!(five_hundred.matches(&parse_ncep_level("500 mb")));
        assert!(!five_hundred.matches(&parse_ncep_level("850 mb")));
        // …and a query naming a value is not matched by a level that has none.
        assert!(!five_hundred.matches(&LevelSpec::named(Surface::Isobaric)));
    }

    /// A layer query is not satisfied by a single level on the same surface,
    /// which is what would happen if the second bound were ignored.
    #[test]
    fn a_layer_query_is_not_matched_by_a_single_level() {
        let layer = LevelSpec::between(Surface::HeightAboveGround, 6000.0, 0.0);
        assert!(!layer.matches(&parse_ncep_level("6000 m above ground")));
        assert!(layer.matches(&parse_ncep_level("6000-0 m above ground")));
    }
}
