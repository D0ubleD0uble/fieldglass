//! Whether two fields are the two components of one vector (#1049).
//!
//! Arrows are drawn from any two fields on one grid, and nothing about two
//! arrays of numbers says they are a u and a v. Wind speed paired with wind
//! direction draws arrows as confident as a real pair's. What says it is the
//! parameter each field names, so this reads the names.
//!
//! # By the name, not by a list of codes
//!
//! A u/v pair is a name with a direction word in it and a partner that says
//! the same thing with the other word: `u-component of wind` and
//! `v-component of wind`, `Eastward sea water velocity` and `Northward sea
//! water velocity`, `10 metre U wind component` and `10 metre V wind
//! component`. Read from the names the tables already resolve, one rule
//! covers every table the workspace carries (the WMO GRIB2 master tables, the
//! NCEP, ECMWF and DWD local GRIB2 tables, ON388 Table 2 and every ECMWF
//! GRIB1 local table) and the pairs a table regeneration adds, where a
//! hand-kept list of code pairs would cover the ones someone remembered.
//!
//! "Covers" has a limit, and the sweeps in `fieldglass-grib1` and
//! `fieldglass-grib2` (`tests/vector_pairs.rs`) state it: every name in those
//! tables that holds a direction word either finds its partner or is on a
//! reviewed list of names that have none or are not components. A
//! regeneration that adds a name in a spelling this rule does not read fails
//! there rather than going unpaired in silence.
//!
//! The direction words come in five spellings, and a pair uses one spelling
//! on both sides, so `Eastward current` (GRIB2 10-1-5) pairs with `Northward
//! current` (10-1-6) and not with `v-component of current` (10-1-3):
//!
//! | u | v |
//! |---|---|
//! | `u` (`u-component`, `U wind`) | `v` |
//! | `eastward` | `northward` |
//! | `zonal`, `longitudinal` | `meridional` |
//! | `east-west` | `north-south` |
//! | `x` (`X-component`) | `y` |
//!
//! `longitudinal` is ECMWF's: its table 172 pairs `Longitudinal component of
//! gravity wave stress` with `Meridional component of gravity wave stress`.
//!
//! CF standard names ([`pairing_of_standard_names`]) use only `eastward` /
//! `northward` and `x` / `y` (`eastward_wind`, `x_wind`,
//! `surface_downward_eastward_stress`, `sea_water_x_velocity`), so only those
//! two count there.
//!
//! A name is compared word by word with its case folded: everything but
//! letters and digits separates words, so `U-component` and `u component` are
//! the same words. A name that holds direction words for both axes
//! (`Covariance of v component/u component`) is not one component, and a name
//! with none is not a component at all.
//!
//! # Not every pair of names is a vector
//!
//! Some tables name a scalar statistic of one component, and its partner the
//! same statistic of the other: `Variance of u component` and `Variance of v
//! component`, `U*U product` and `V*V product`, `Latitude of U Wind Component
//! of Velocity`. Two variances are not a vector, so a parameter name that
//! starts `Latitude of`, `Longitude of`, `Variance of` or `Standard deviation
//! of`, holds the word `variance`, is a `*` product, or is one of the Hanna
//! dispersion statistics (`Zonal velocity fluctuation (Hanna)`) is not a
//! component.
//!
//! CF standard names have per-axis statistics of their own, and the same
//! holds for them: a standard name followed by one of the four modifiers of
//! CF 1.11 Appendix C (`eastward_wind standard_error`, `northward_wind
//! number_of_observations`, `detection_minimum`, `status_flag`) names a
//! quantity about the component rather than the component, and so does one
//! that holds `square_of_` or `variance_of_` (`square_of_eastward_wind`).
//!
//! # Not every field has a name to read
//!
//! A GRIB parameter no table names, and a NetCDF or Zarr variable with no
//! `standard_name`, give the rule nothing to read, so the answer for them is
//! [`VectorPairing::Unknown`] rather than a claim either way. So do two kinds
//! of name that are there but say nothing:
//!
//! - a table's placeholder for a code it reserves or leaves open
//!   ([`is_placeholder_name`]): `Experimental product`, `Reserved`, `Reserved
//!   for future unbalanced components`, `Indicates a missing value`,
//!   `Missing`, and ECMWF's `Wave experimental parameter 1` to `5`;
//! - a `standard_name` that is not a standard name, one modifier: more than
//!   two words, or a second word that is not a CF modifier (`eastward wind`).
//!
//! The exception is a partner whose name is read and is not a component
//! (`Temperature`): no name for the other field makes that a vector, so the
//! pair is [`VectorPairing::NotAPair`].
//!
//! What the rule cannot know is that the two fields are at the same level, or
//! valid at the same time. It answers only whether their parameters pair.

/// Whether a u field and a v field are the two components of one vector.
///
/// The answer to a pairing a caller has already made, so it is never a
/// refusal: arrows are drawn whichever this is, and a host says so beside
/// them when it is [`VectorPairing::Swapped`] or [`VectorPairing::NotAPair`].
/// [`VectorPairing::Unknown`] says nothing either way, so a host says nothing
/// beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorPairing {
    /// The u field is the eastward (or along-x) component and the v field is
    /// its northward (or along-y) partner.
    Recognised,
    /// The two are a pair, but the u field is the northward component and the
    /// v field the eastward one: the arrows are drawn reflected about the
    /// diagonal.
    Swapped,
    /// The two are not the components of one vector: one of them is not a
    /// component, they are components of different things, or both are the
    /// same component.
    NotAPair,
    /// Can't tell: a field has no name to read (a GRIB parameter no table
    /// names or one its table names only with a placeholder, or a variable
    /// with no usable `standard_name`), and its partner's name does not
    /// settle it.
    Unknown,
}

/// Which component a name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    U,
    V,
}

/// One direction word, and the spelling family it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    Uv,
    EastwardNorthward,
    ZonalMeridional,
    EastWestNorthSouth,
    Xy,
}

/// A direction word or phrase: the words it is spelled with, in order.
type Direction = (&'static [&'static str], Spelling, Axis);

/// The direction words GRIB parameter tables use. A phrase is matched before
/// any single word.
const PARAMETER_WORDS: &[Direction] = &[
    (&["east", "west"], Spelling::EastWestNorthSouth, Axis::U),
    (&["north", "south"], Spelling::EastWestNorthSouth, Axis::V),
    (&["u"], Spelling::Uv, Axis::U),
    (&["v"], Spelling::Uv, Axis::V),
    (&["eastward"], Spelling::EastwardNorthward, Axis::U),
    (&["northward"], Spelling::EastwardNorthward, Axis::V),
    (&["zonal"], Spelling::ZonalMeridional, Axis::U),
    (&["longitudinal"], Spelling::ZonalMeridional, Axis::U),
    (&["meridional"], Spelling::ZonalMeridional, Axis::V),
    (&["x"], Spelling::Xy, Axis::U),
    (&["y"], Spelling::Xy, Axis::V),
];

/// The direction words CF standard names use.
const STANDARD_NAME_WORDS: &[Direction] = &[
    (&["eastward"], Spelling::EastwardNorthward, Axis::U),
    (&["northward"], Spelling::EastwardNorthward, Axis::V),
    (&["x"], Spelling::Xy, Axis::U),
    (&["y"], Spelling::Xy, Axis::V),
];

/// One word of a name, with a direction word reduced to its spelling so the
/// two partners compare equal.
#[derive(Debug, PartialEq, Eq)]
enum Word {
    Direction(Spelling),
    Other(String),
}

/// A name read as a vector component: which axis, and the words it shares
/// with its partner.
#[derive(Debug)]
struct Component {
    axis: Axis,
    words: Vec<Word>,
}

/// A name's words, case folded.
fn words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The direction a name holds at word `at`, and how many words it spans.
fn direction_at(
    words: &[String],
    at: usize,
    vocabulary: &[Direction],
) -> Option<(Spelling, Axis, usize)> {
    vocabulary.iter().find_map(|&(phrase, spelling, axis)| {
        let span = words.get(at..at + phrase.len())?;
        span.iter()
            .zip(phrase)
            .all(|(w, p)| w == p)
            .then_some((spelling, axis, phrase.len()))
    })
}

fn component(name: &str, vocabulary: &[Direction]) -> Option<Component> {
    let folded = words(name);
    let mut axis = None;
    let mut out = Vec::new();
    let mut at = 0;
    while at < folded.len() {
        match direction_at(&folded, at, vocabulary) {
            Some((spelling, a, span)) => {
                if axis.is_some_and(|seen| seen != a) {
                    return None;
                }
                axis = Some(a);
                out.push(Word::Direction(spelling));
                at += span;
            }
            None => {
                out.push(Word::Other(folded[at].clone()));
                at += 1;
            }
        }
    }
    axis.map(|axis| Component { axis, words: out })
}

/// Whether a parameter name is a scalar statistic of one component rather
/// than a component: see the module docs.
fn is_per_axis_statistic(name: &str) -> bool {
    let folded = words(name);
    let starts = |prefix: &[&str]| {
        folded.len() > prefix.len() && folded.iter().zip(prefix).all(|(w, p)| w == p)
    };
    name.contains('*')
        || starts(&["latitude", "of"])
        || starts(&["longitude", "of"])
        || starts(&["variance", "of"])
        || starts(&["standard", "deviation", "of"])
        || folded.iter().any(|w| w == "variance" || w == "hanna")
}

/// The modifiers CF lets follow a standard name (CF 1.11 Appendix C). Each
/// names a quantity about the variable, not the variable.
const STANDARD_NAME_MODIFIERS: [&str; 4] = [
    "detection_minimum",
    "number_of_observations",
    "standard_error",
    "status_flag",
];

/// Whether a bare CF standard name (no modifier) is a statistic of one
/// component rather than a component: see the module docs.
fn is_per_axis_standard_name_statistic(name: &str) -> bool {
    let folded = words(name);
    let holds = |phrase: &[&str]| {
        folded
            .windows(phrase.len())
            .any(|w| w.iter().zip(phrase).all(|(a, b)| a == b))
    };
    holds(&["square", "of"]) || holds(&["variance", "of"])
}

/// Whether a GRIB parameter name is a table's placeholder for a code it
/// reserves or leaves open, rather than the name of a quantity: see the
/// module docs. Such a name says nothing about whether the field is a
/// component, so it is read as no name at all.
///
/// The spellings are the ones the workspace's tables carry, swept by
/// `tests/vector_pairs.rs` in `fieldglass-grib1` and `fieldglass-grib2`, so a
/// table regeneration that adds another fails there.
///
/// ```
/// use fieldglass_core::vector_pair::is_placeholder_name;
///
/// assert!(is_placeholder_name("Experimental product"));
/// assert!(is_placeholder_name("Reserved for future unbalanced components"));
/// assert!(is_placeholder_name("Wave experimental parameter 3"));
/// assert!(!is_placeholder_name("u-component of wind"));
/// ```
#[must_use]
pub fn is_placeholder_name(name: &str) -> bool {
    let folded = words(name);
    let w: Vec<&str> = folded.iter().map(String::as_str).collect();
    matches!(
        w.as_slice(),
        [] | ["reserved", ..]
            | ["experimental", "product"]
            | ["missing"]
            | ["missing", "value"]
            | ["indicates", "a", "missing", "value"]
    ) || w.windows(3).any(|t| {
        t[0] == "experimental" && t[1] == "parameter" && t[2].bytes().all(|b| b.is_ascii_digit())
    })
}

/// A name as read: `None` when the field has no name, else the component it
/// is, if it is one.
type Read = Option<Option<Component>>;

fn pairing(u: Read, v: Read) -> VectorPairing {
    match (u, v) {
        (Some(Some(u)), Some(Some(v))) if u.words == v.words && u.axis != v.axis => {
            if u.axis == Axis::U {
                VectorPairing::Recognised
            } else {
                VectorPairing::Swapped
            }
        }
        // Both read, or one read as no component at all: no name the other
        // could have makes them a vector.
        (Some(_), Some(_)) | (Some(None), None) | (None, Some(None)) => VectorPairing::NotAPair,
        _ => VectorPairing::Unknown,
    }
}

/// A GRIB parameter name read as a component, or `None`.
fn parameter_component(name: &str) -> Option<Component> {
    if is_per_axis_statistic(name) {
        return None;
    }
    component(name, PARAMETER_WORDS)
}

/// Whether a GRIB parameter name holds a word this rule reads as a direction,
/// whether or not it then reads as a component.
///
/// For a sweep over a table: a name for which this is `true` and that pairs
/// with no other name is either one the rule should learn to read or one to
/// say has no partner.
#[must_use]
pub fn mentions_a_direction(name: &str) -> bool {
    let folded = words(name);
    (0..folded.len()).any(|at| direction_at(&folded, at, PARAMETER_WORDS).is_some())
}

/// Whether two GRIB parameters, by the names their tables give them, are a
/// u/v pair in this order.
///
/// `None` is a field whose parameter no table names, or that has no
/// parameter: the answer is then [`VectorPairing::Unknown`], unless the other
/// name is read and is not a component. A host passes `None` for a parameter
/// its tables do not resolve, not the placeholder it displays for one
/// (`Parameter 0/2/250`), which this would read as a name.
///
/// ```
/// use fieldglass_core::vector_pair::{VectorPairing, pairing_of_parameter_names};
///
/// let u = Some("u-component of wind");
/// let v = Some("v-component of wind");
/// assert_eq!(pairing_of_parameter_names(u, v), VectorPairing::Recognised);
/// assert_eq!(pairing_of_parameter_names(v, u), VectorPairing::Swapped);
/// assert_eq!(
///     pairing_of_parameter_names(Some("Wind speed"), Some("Wind direction")),
///     VectorPairing::NotAPair,
/// );
/// assert_eq!(pairing_of_parameter_names(u, None), VectorPairing::Unknown);
/// assert_eq!(
///     pairing_of_parameter_names(Some("Temperature"), None),
///     VectorPairing::NotAPair,
/// );
/// ```
#[must_use]
pub fn pairing_of_parameter_names(u: Option<&str>, v: Option<&str>) -> VectorPairing {
    let read = |name: Option<&str>| {
        name.filter(|n| !is_placeholder_name(n))
            .map(parameter_component)
    };
    pairing(read(u), read(v))
}

/// A `standard_name` attribute, read: `None` when it is not one standard
/// name with at most one CF modifier, else the component it is, if any.
fn read_standard_name(attribute: &str) -> Read {
    let mut parts = attribute.split_whitespace();
    let (Some(name), modifier, None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    match modifier {
        None if is_per_axis_standard_name_statistic(name) => Some(None),
        None => Some(component(name, STANDARD_NAME_WORDS)),
        Some(m) if STANDARD_NAME_MODIFIERS.contains(&m) => Some(None),
        Some(_) => None,
    }
}

/// Whether two CF standard names are a u/v pair in this order:
/// `eastward_*` with `northward_*`, or `x_*` with `y_*`, the rest of the name
/// the same.
///
/// `None` is a variable with no `standard_name`, read as
/// [`pairing_of_parameter_names`] reads a missing parameter name; so is an
/// attribute that is not one standard name with at most one CF modifier.
///
/// ```
/// use fieldglass_core::vector_pair::{VectorPairing, pairing_of_standard_names};
///
/// let (u, v) = (Some("eastward_wind"), Some("northward_wind"));
/// assert_eq!(pairing_of_standard_names(u, v), VectorPairing::Recognised);
/// assert_eq!(pairing_of_standard_names(v, u), VectorPairing::Swapped);
/// assert_eq!(
///     pairing_of_standard_names(u, Some("y_wind")),
///     VectorPairing::NotAPair,
/// );
/// assert_eq!(pairing_of_standard_names(u, None), VectorPairing::Unknown);
/// ```
#[must_use]
pub fn pairing_of_standard_names(u: Option<&str>, v: Option<&str>) -> VectorPairing {
    pairing(
        u.and_then(read_standard_name),
        v.and_then(read_standard_name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(u: &str, v: &str) -> VectorPairing {
        pairing_of_parameter_names(Some(u), Some(v))
    }

    #[test]
    fn each_spelling_pairs_with_its_own_partner_only() {
        for (u, v) in [
            ("u-component of wind", "v-component of wind"),
            ("U component of wind", "V component of wind"),
            ("10 metre U wind component", "10 metre V wind component"),
            ("Eastward current", "Northward current"),
            (
                "Adiabatic tendency of zonal wind",
                "Adiabatic tendency of meridional wind",
            ),
            (
                "convective momentum tendency (X-component)",
                "convective momentum tendency (Y-component)",
            ),
            (
                "Q-Vektor X-Komponente (geostrophisch)",
                "Q-Vektor Y-Komponente (geostrophisch)",
            ),
        ] {
            assert_eq!(names(u, v), VectorPairing::Recognised, "{u} / {v}");
            assert_eq!(names(v, u), VectorPairing::Swapped, "{v} / {u}");
        }
        // A spelling of one family does not pair with another's partner.
        assert_eq!(
            names("Eastward current", "v-component of current"),
            VectorPairing::NotAPair
        );
        assert_eq!(
            names("u-component of wind", "Northward wind"),
            VectorPairing::NotAPair
        );
    }

    #[test]
    fn east_west_north_south_and_longitudinal() {
        for (u, v) in [
            ("East-West surface stress", "North-South surface stress"),
            (
                "East-West gravity wave drag tendency anomaly",
                "North-South gravity wave drag tendency anomaly",
            ),
            (
                "Longitudinal component of gravity wave stress",
                "Meridional component of gravity wave stress",
            ),
            (
                "Longitudinal component of gravity wave stress (variable resolution)",
                "Meridional component of gravity wave stress (variable resolution)",
            ),
        ] {
            assert_eq!(names(u, v), VectorPairing::Recognised, "{u} / {v}");
            assert_eq!(names(v, u), VectorPairing::Swapped, "{v} / {u}");
        }
        // The two-word spellings are phrases: one word of them is not a
        // direction, and a diagonal is not one either.
        assert_eq!(
            names("East surface stress", "North surface stress"),
            VectorPairing::NotAPair
        );
        assert!(!mentions_a_direction(
            "North-East/South-West component of sub-gridscale orography"
        ));
        assert!(mentions_a_direction("East-West surface stress"));
        // `longitudinal` and `zonal` are one family, so they never pair with each other.
        assert_eq!(
            names(
                "Longitudinal component of gravity wave stress",
                "Zonal component of gravity wave stress"
            ),
            VectorPairing::NotAPair
        );
    }

    #[test]
    fn per_axis_statistics_are_not_components() {
        for (u, v) in [
            (
                "Latitude of U Wind Component of Velocity",
                "Latitude of V Wind Component of Velocity",
            ),
            (
                "Longitude of U Wind Component of Velocity",
                "Longitude of V Wind Component of Velocity",
            ),
            ("Variance of u component", "Variance of v component"),
            (
                "Standard deviation of U velocity",
                "Standard deviation of V velocity",
            ),
            ("U*U product", "V*V product"),
            ("U*T product", "V*T product"),
            (
                "Zonal Lagrangian time scale (direct) (Hanna)",
                "Meridional Lagrangian time scale (direct) (Hanna)",
            ),
            (
                "Zonal velocity fluctuation (Hanna)",
                "Meridional velocity fluctuation (Hanna)",
            ),
            (
                "East-West component of sub-gridscale orographic variance",
                "North-South component of sub-gridscale orographic variance",
            ),
        ] {
            assert_eq!(names(u, v), VectorPairing::NotAPair, "{u} / {v}");
            assert_eq!(names(v, u), VectorPairing::NotAPair, "{v} / {u}");
        }
        // Each stop is a whole leading phrase or word, not a substring: a
        // covariance of a component is still one, and so is a name that only
        // mentions latitude later on.
        assert_eq!(
            names(
                "Covariance of U component and temperature",
                "Covariance of V component and temperature"
            ),
            VectorPairing::Recognised
        );
        assert_eq!(
            names(
                "u-component of wind at latitude of 30",
                "v-component of wind at latitude of 30"
            ),
            VectorPairing::Recognised
        );
    }

    #[test]
    fn case_and_punctuation_do_not_separate_a_pair() {
        assert_eq!(
            names("Momentum flux, u component", "Momentum flux, v-component"),
            VectorPairing::Recognised
        );
        assert_eq!(
            names("U-component of wind", "v-component of wind"),
            VectorPairing::Recognised
        );
    }

    #[test]
    fn what_is_not_a_pair() {
        // Not components at all: no direction word.
        assert_eq!(
            names("Wind speed", "Wind direction"),
            VectorPairing::NotAPair
        );
        assert_eq!(names("Wind speed", "Wind speed"), VectorPairing::NotAPair);
        assert_eq!(
            names("Temperature", "v-component of wind"),
            VectorPairing::NotAPair
        );
        // The same component twice.
        assert_eq!(
            names("u-component of wind", "u-component of wind"),
            VectorPairing::NotAPair
        );
        // Components of two different vectors.
        assert_eq!(
            names("u-component of wind", "v-component storm motion"),
            VectorPairing::NotAPair
        );
        assert_eq!(
            names("10 metre U wind component", "V component of wind"),
            VectorPairing::NotAPair
        );
        // A name with both axes in it is not one component.
        assert_eq!(
            names(
                "Covariance of v component/u component",
                "Covariance of v component/u component"
            ),
            VectorPairing::NotAPair
        );
        // A direction letter inside a word is not a direction word.
        assert_eq!(
            names("Visibility", "Ultraviolet index"),
            VectorPairing::NotAPair
        );
    }

    #[test]
    fn a_missing_name_is_unknown_unless_its_partner_settles_it() {
        // Nothing to read: can't tell, in either order.
        for (u, v) in [
            (None, Some("v-component of wind")),
            (Some("u-component of wind"), None),
            (None, None),
        ] {
            assert_eq!(
                pairing_of_parameter_names(u, v),
                VectorPairing::Unknown,
                "{u:?} / {v:?}"
            );
            assert_eq!(
                pairing_of_standard_names(u.map(|_| "eastward_wind"), v.map(|_| "northward_wind")),
                VectorPairing::Unknown,
                "standard names {u:?} / {v:?}"
            );
        }
        // A partner that is read and is not a component makes no vector,
        // whatever the other is.
        for (u, v) in [
            (None, Some("Temperature")),
            (Some("Wind speed"), None),
            (None, Some("Variance of v component")),
        ] {
            assert_eq!(
                pairing_of_parameter_names(u, v),
                VectorPairing::NotAPair,
                "{u:?} / {v:?}"
            );
        }
        assert_eq!(
            pairing_of_standard_names(None, Some("air_temperature")),
            VectorPairing::NotAPair
        );
        assert_eq!(
            pairing_of_standard_names(Some("square_of_eastward_wind"), None),
            VectorPairing::NotAPair
        );
    }

    #[test]
    fn cf_standard_names() {
        for (u, v) in [
            ("eastward_wind", "northward_wind"),
            ("x_wind", "y_wind"),
            (
                "eastward_sea_water_velocity",
                "northward_sea_water_velocity",
            ),
            (
                "surface_downward_eastward_stress",
                "surface_downward_northward_stress",
            ),
            ("sea_ice_x_velocity", "sea_ice_y_velocity"),
            ("sea_water_x_velocity", "sea_water_y_velocity"),
            ("tendency_of_eastward_wind", "tendency_of_northward_wind"),
        ] {
            assert_eq!(
                pairing_of_standard_names(Some(u), Some(v)),
                VectorPairing::Recognised,
                "{u} / {v}"
            );
            assert_eq!(
                pairing_of_standard_names(Some(v), Some(u)),
                VectorPairing::Swapped,
                "{v} / {u}"
            );
        }
        for (u, v) in [
            // Different suffixes.
            ("eastward_wind", "northward_sea_water_velocity"),
            // Two spellings.
            ("eastward_wind", "y_wind"),
            // Not components.
            ("wind_speed", "wind_from_direction"),
            ("air_temperature", "air_temperature"),
            // The same component twice.
            ("eastward_wind", "eastward_wind"),
            // CF writes no u/v or zonal/meridional direction words.
            ("u_wind", "v_wind"),
        ] {
            assert_eq!(
                pairing_of_standard_names(Some(u), Some(v)),
                VectorPairing::NotAPair,
                "{u} / {v}"
            );
        }
    }

    #[test]
    fn cf_per_axis_statistics_are_not_components() {
        for (u0, v0) in [("eastward_wind", "northward_wind"), ("x_wind", "y_wind")] {
            for modifier in STANDARD_NAME_MODIFIERS {
                let (u, v) = (format!("{u0} {modifier}"), format!("{v0} {modifier}"));
                for (a, b) in [(&*u, &*v), (&*v, &*u), (&*u, v0), (u0, &*v)] {
                    assert_eq!(
                        pairing_of_standard_names(Some(a), Some(b)),
                        VectorPairing::NotAPair,
                        "{a} / {b}"
                    );
                }
            }
            for prefix in ["square_of_", "variance_of_", "tendency_of_square_of_"] {
                let (u, v) = (format!("{prefix}{u0}"), format!("{prefix}{v0}"));
                assert_eq!(
                    pairing_of_standard_names(Some(&u), Some(&v)),
                    VectorPairing::NotAPair,
                    "{u} / {v}"
                );
            }
            // The plain names still pair.
            assert_eq!(
                pairing_of_standard_names(Some(u0), Some(v0)),
                VectorPairing::Recognised,
                "{u0} / {v0}"
            );
        }
        // Whitespace around a name is not a modifier.
        assert_eq!(
            pairing_of_standard_names(Some("  eastward_wind "), Some("northward_wind\n")),
            VectorPairing::Recognised
        );
    }

    #[test]
    fn a_standard_name_that_is_not_one_is_unknown() {
        for (u, v) in [
            // Words where CF writes underscores: no second word is a modifier.
            ("eastward wind", "northward wind"),
            ("eastward_wind anomaly", "northward_wind anomaly"),
            // A modifier spelled with spaces, or two of them.
            (
                "eastward_wind standard error",
                "northward_wind standard error",
            ),
            (
                "eastward_wind standard_error status_flag",
                "northward_wind standard_error status_flag",
            ),
            // A modifier is lower case, as every standard name is.
            (
                "eastward_wind Standard_Error",
                "northward_wind Standard_Error",
            ),
        ] {
            assert_eq!(
                pairing_of_standard_names(Some(u), Some(v)),
                VectorPairing::Unknown,
                "{u} / {v}"
            );
        }
        // Against a partner read as no component, still not a pair.
        assert_eq!(
            pairing_of_standard_names(Some("eastward wind"), Some("air_temperature")),
            VectorPairing::NotAPair
        );
        // An empty or blank attribute says nothing.
        assert_eq!(
            pairing_of_standard_names(Some(" "), Some("northward_wind")),
            VectorPairing::Unknown
        );
    }

    #[test]
    fn a_placeholder_name_is_no_name() {
        for placeholder in [
            "Experimental product",
            "Reserved",
            "Reserved for future unbalanced components",
            "Indicates a missing value",
            "Missing",
            "Missing value",
            "Wave experimental parameter 1",
            "Wave experimental parameter 5",
            "",
        ] {
            assert!(is_placeholder_name(placeholder), "{placeholder:?}");
            // ECMWF table 128 ids 80 and 81 are both `Experimental product`.
            assert_eq!(
                names(placeholder, placeholder),
                VectorPairing::Unknown,
                "{placeholder:?} twice"
            );
            assert_eq!(
                names("u-component of wind", placeholder),
                VectorPairing::Unknown,
                "{placeholder:?} as v"
            );
            assert_eq!(
                names(placeholder, "Temperature"),
                VectorPairing::NotAPair,
                "{placeholder:?} with a named non-component"
            );
        }
        // A real name that holds one of the words is not a placeholder.
        for name in [
            "Experimental product index",
            "Probability of missing data",
            "Wave experimental parameter",
            "u-component of wind",
            "Sky transparency index",
        ] {
            assert!(!is_placeholder_name(name), "{name:?}");
        }
    }
}
