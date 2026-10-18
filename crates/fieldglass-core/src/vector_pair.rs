//! Whether two fields are the two components of one vector (#1049).
//!
//! Arrows are drawn from any two fields on one grid, and nothing about two
//! arrays of numbers says they are a u and a v. Wind speed paired with wind
//! direction draws arrows as confident as a real pair's. What says it is the
//! parameter each field names, so this reads the names.
//!
//! # By the name, not by a list of codes
//!
//! A u/v pair is a name with one direction word in it and a partner that says
//! the same thing with the other word: `u-component of wind` and
//! `v-component of wind`, `Eastward sea water velocity` and `Northward sea
//! water velocity`, `10 metre U wind component` and `10 metre V wind
//! component`. That is the concept, and it is what every parameter table in
//! the workspace writes: the WMO GRIB2 master tables, the NCEP, ECMWF and DWD
//! local GRIB2 tables, ON388 Table 2 and every ECMWF GRIB1 local table. Read
//! from the names the tables already resolve, one rule covers all of them,
//! including the pairs a table regeneration adds, where a hand-kept list of
//! code pairs would cover the ones someone remembered.
//!
//! The direction words come in four spellings, and a pair uses one spelling
//! on both sides, so `Eastward current` (GRIB2 10-1-5) pairs with `Northward
//! current` (10-1-6) and not with `v-component of current` (10-1-3):
//!
//! | u | v |
//! |---|---|
//! | `u` (`u-component`, `U wind`) | `v` |
//! | `eastward` | `northward` |
//! | `zonal` | `meridional` |
//! | `x` (`X-component`) | `y` |
//!
//! CF standard names ([`pairing_of_standard_names`]) use only `eastward` /
//! `northward` and `x` / `y` (`eastward_wind`, `x_wind`,
//! `surface_downward_eastward_stress`, `sea_ice_x_velocity`), so only those
//! two count there.
//!
//! A name is compared word by word with its case folded: everything but
//! letters and digits separates words, so `U-component` and `u component` are
//! the same words. A name that holds direction words for both axes
//! (`Covariance of v component/u component`) is not one component, and a name
//! with none is not a component at all.
//!
//! What the rule cannot know is that the two fields are at the same level, or
//! valid at the same time. It answers only whether their parameters pair.

/// Whether a u field and a v field are the two components of one vector.
///
/// The answer to a pairing a caller has already made, so it is never a
/// refusal: arrows are drawn whichever this is, and a host says so beside
/// them when it is not [`VectorPairing::Recognised`].
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
    Xy,
}

/// The direction words GRIB parameter tables use.
const PARAMETER_WORDS: &[(&str, Spelling, Axis)] = &[
    ("u", Spelling::Uv, Axis::U),
    ("v", Spelling::Uv, Axis::V),
    ("eastward", Spelling::EastwardNorthward, Axis::U),
    ("northward", Spelling::EastwardNorthward, Axis::V),
    ("zonal", Spelling::ZonalMeridional, Axis::U),
    ("meridional", Spelling::ZonalMeridional, Axis::V),
    ("x", Spelling::Xy, Axis::U),
    ("y", Spelling::Xy, Axis::V),
];

/// The direction words CF standard names use.
const STANDARD_NAME_WORDS: &[(&str, Spelling, Axis)] = &[
    ("eastward", Spelling::EastwardNorthward, Axis::U),
    ("northward", Spelling::EastwardNorthward, Axis::V),
    ("x", Spelling::Xy, Axis::U),
    ("y", Spelling::Xy, Axis::V),
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

fn component(name: &str, vocabulary: &[(&str, Spelling, Axis)]) -> Option<Component> {
    let mut axis = None;
    let mut words = Vec::new();
    for word in name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let folded = word.to_lowercase();
        match vocabulary.iter().find(|(w, _, _)| *w == folded) {
            Some(&(_, spelling, a)) => {
                if axis.is_some_and(|seen| seen != a) {
                    return None;
                }
                axis = Some(a);
                words.push(Word::Direction(spelling));
            }
            None => words.push(Word::Other(folded)),
        }
    }
    axis.map(|axis| Component { axis, words })
}

fn pairing(u: Option<Component>, v: Option<Component>) -> VectorPairing {
    match (u, v) {
        (Some(u), Some(v)) if u.words == v.words && u.axis != v.axis => {
            if u.axis == Axis::U {
                VectorPairing::Recognised
            } else {
                VectorPairing::Swapped
            }
        }
        _ => VectorPairing::NotAPair,
    }
}

/// Whether two GRIB parameters, by the names their tables give them, are a
/// u/v pair in this order.
///
/// `None` is a field whose parameter no table names; it pairs with nothing.
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
/// ```
#[must_use]
pub fn pairing_of_parameter_names(u: Option<&str>, v: Option<&str>) -> VectorPairing {
    let read = |name: Option<&str>| name.and_then(|n| component(n, PARAMETER_WORDS));
    pairing(read(u), read(v))
}

/// Whether two CF standard names are a u/v pair in this order:
/// `eastward_*` with `northward_*`, or `x_*` with `y_*`, the rest of the name
/// the same.
///
/// ```
/// use fieldglass_core::vector_pair::{VectorPairing, pairing_of_standard_names};
///
/// assert_eq!(
///     pairing_of_standard_names("eastward_wind", "northward_wind"),
///     VectorPairing::Recognised,
/// );
/// assert_eq!(pairing_of_standard_names("y_wind", "x_wind"), VectorPairing::Swapped);
/// assert_eq!(
///     pairing_of_standard_names("eastward_wind", "y_wind"),
///     VectorPairing::NotAPair,
/// );
/// ```
#[must_use]
pub fn pairing_of_standard_names(u: &str, v: &str) -> VectorPairing {
    pairing(
        component(u, STANDARD_NAME_WORDS),
        component(v, STANDARD_NAME_WORDS),
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
        // A parameter no table names pairs with nothing.
        assert_eq!(
            pairing_of_parameter_names(None, Some("v-component of wind")),
            VectorPairing::NotAPair
        );
        assert_eq!(
            pairing_of_parameter_names(None, None),
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
            ("tendency_of_eastward_wind", "tendency_of_northward_wind"),
        ] {
            assert_eq!(
                pairing_of_standard_names(u, v),
                VectorPairing::Recognised,
                "{u} / {v}"
            );
            assert_eq!(
                pairing_of_standard_names(v, u),
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
                pairing_of_standard_names(u, v),
                VectorPairing::NotAPair,
                "{u} / {v}"
            );
        }
    }
}
